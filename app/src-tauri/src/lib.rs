//! Fletcher's Tauri shell: thin command layer over fletcher-core (ADR-0001/0005 —
//! the UI draws, the core thinks). DTOs live here so the core stays serde-free.

use fletcher_core::config::{render_fletcher_file, ChainFilter, ConfigDoc, FilterKind, Parsed};
use fletcher_core::dsp::{auto_preamp_db, chain_response_db, log_freqs, FilterSpec};
use fletcher_core::presets::{sanitize_name, PresetStore};
use fletcher_core::{apo, devices, dsp, fsx};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

fn data_dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Fletcher")
}

fn store() -> Result<PresetStore, String> {
    PresetStore::open(&data_dir().join("presets")).map_err(|e| e.to_string())
}

fn read_state() -> serde_json::Value {
    std::fs::read_to_string(data_dir().join("state.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

fn write_state_field(key: &str, value: serde_json::Value) {
    let _ = std::fs::create_dir_all(data_dir());
    let mut state = read_state();
    state[key] = value;
    let _ = fsx::write_atomic(&data_dir().join("state.json"), &state.to_string());
}

fn active_preset() -> Option<String> {
    read_state().get("activePreset")?.as_str().map(String::from)
}

fn set_active_preset(name: Option<&str>) {
    write_state_field("activePreset", serde_json::json!(name));
}

/// The global reference loudness (dB) everything is normalized to.
/// Default −8; Settings' noise-calibration flow will own this (Q-16).
fn reference_db() -> f64 {
    read_state()
        .get("referenceDb")
        .and_then(|v| v.as_f64())
        .unwrap_or(-8.0)
}

fn ab_side() -> String {
    read_state()
        .get("abSide")
        .and_then(|v| v.as_str())
        .unwrap_or("a")
        .to_string()
}

fn set_ab_side(side: &str) {
    write_state_field("abSide", serde_json::json!(side));
}

const FS: f64 = 48000.0;
const CURVE_POINTS: usize = 200;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EqState {
    device_name: Option<String>,
    preamp_db: f64,
    freqs: Vec<f64>,
    sum_db: Vec<f64>,
    filters: Vec<EqFilter>,
    source_files: Vec<String>,
    /// Every Include in config.txt, contributing or not (external EQs show
    /// in the preset menu even when currently inactive).
    includes: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EqFilter {
    enabled: bool,
    kind: &'static str,
    fc_hz: f64,
    gain_db: f64,
    q: f64,
    response_db: Vec<f64>,
    /// Which config file this filter came from; only "fletcher.txt" is editable.
    source_file: String,
}

/// The live EQ chain: every filter and preamp reachable from config.txt
/// (one include level), with computed frequency responses.
#[tauri::command]
fn eq_state() -> Result<EqState, String> {
    let install = apo::detect().map_err(|e| e.to_string())?;
    let root_path = install.config_path.join("config.txt");
    let root_text = std::fs::read_to_string(&root_path).map_err(|e| e.to_string())?;
    let root = ConfigDoc::parse(&root_text);

    let mut docs: Vec<(String, ConfigDoc)> = vec![("config.txt".into(), root)];
    let includes: Vec<String> = docs[0]
        .1
        .directives()
        .filter_map(|d| match d {
            Parsed::Include { path } => Some(path.clone()),
            _ => None,
        })
        .collect();
    for inc in includes {
        if let Ok(text) = std::fs::read_to_string(install.config_path.join(&inc)) {
            docs.push((inc, ConfigDoc::parse(&text)));
        }
    }

    let mut preamp_db = 0.0;
    let mut filters: Vec<(bool, FilterKind, f64, f64, f64, String)> = Vec::new();
    let mut source_files = Vec::new();
    for (name, doc) in &docs {
        let mut contributed = false;
        for d in doc.directives() {
            match d {
                Parsed::Preamp { db } => {
                    preamp_db += db;
                    contributed = true;
                }
                Parsed::Filter {
                    enabled,
                    kind,
                    fc_hz,
                    gain_db,
                    q,
                    ..
                } => {
                    filters.push((
                        *enabled,
                        *kind,
                        fc_hz.unwrap_or(1000.0),
                        gain_db.unwrap_or(0.0),
                        q.unwrap_or(std::f64::consts::FRAC_1_SQRT_2),
                        name.clone(),
                    ));
                    contributed = true;
                }
                _ => {}
            }
        }
        if contributed {
            source_files.push(name.clone());
        }
    }

    let freqs = log_freqs(20.0, 20000.0, CURVE_POINTS);
    let specs: Vec<FilterSpec> = filters
        .iter()
        .filter(|f| f.0)
        .map(|&(_, kind, fc_hz, gain_db, q, _)| FilterSpec {
            kind,
            fc_hz,
            gain_db,
            q,
        })
        .collect();
    let sum_db = chain_response_db(&specs, preamp_db, FS, &freqs);

    let filter_dtos = filters
        .iter()
        .map(|(enabled, kind, fc_hz, gain_db, q, source)| {
            let biquad = dsp::Biquad::rbj(*kind, FS, *fc_hz, *gain_db, *q);
            EqFilter {
                enabled: *enabled,
                kind: kind.code(),
                fc_hz: *fc_hz,
                gain_db: *gain_db,
                q: *q,
                response_db: freqs.iter().map(|&f| biquad.magnitude_db(f, FS)).collect(),
                source_file: source.clone(),
            }
        })
        .collect();

    let includes = docs.iter().skip(1).map(|(n, _)| n.clone()).collect();
    Ok(EqState {
        device_name: devices::default_render_device_name(),
        preamp_db,
        freqs,
        sum_db,
        filters: filter_dtos,
        source_files,
        includes,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FilterIn {
    enabled: bool,
    kind: String,
    fc_hz: f64,
    gain_db: f64,
    q: f64,
}

/// Validate + clamp incoming filters into a chain — the shared adapter for
/// every command that accepts a chain as an argument.
fn chain_of(filters: &[FilterIn]) -> Result<Vec<ChainFilter>, String> {
    filters
        .iter()
        .map(|f| {
            Ok(ChainFilter {
                enabled: f.enabled,
                kind: FilterKind::from_code(&f.kind)
                    .ok_or_else(|| format!("unknown filter type {}", f.kind))?,
                fc_hz: f.fc_hz.clamp(10.0, 24000.0),
                gain_db: f.gain_db.clamp(-30.0, 30.0),
                q: f.q.clamp(0.01, 100.0),
            })
        })
        .collect()
}

/// Replace Fletcher's own chain (fletcher.txt) with the given filters.
/// Auto-preamp is computed from the summed response peak (TB-06), the file
/// is written atomically (TB-11), and APO hot-reloads it. Returns the fresh
/// merged state.
#[tauri::command]
fn set_fletcher_chain(filters: Vec<FilterIn>) -> Result<EqState, String> {
    let install = apo::detect().map_err(|e| e.to_string())?;

    let chain = chain_of(&filters)?;
    let specs = specs_of(&chain);
    let preamp = matched_preamp(&specs);

    let text = render_fletcher_file(preamp, &chain);
    fsx::write_atomic(&install.config_path.join("fletcher.txt"), &text)
        .map_err(|e| e.to_string())?;

    // Edits persist into the active preset, so switching away and back keeps them.
    if let Some(name) = active_preset() {
        let _ = store()?.save(&name, preamp, &chain);
    }
    // Editing implies listening to the chain: land on A.
    set_ab_side("a");

    eq_state()
}

/// Rewrite fletcher.txt from a chain (with fresh auto-preamp).
fn activate_chain(chain: &[ChainFilter]) -> Result<(), String> {
    let install = apo::detect().map_err(|e| e.to_string())?;
    let specs = specs_of(chain);
    let preamp = matched_preamp(&specs);
    fsx::write_atomic(
        &install.config_path.join("fletcher.txt"),
        &render_fletcher_file(preamp, chain),
    )
    .map_err(|e| e.to_string())
}

/// Every filter currently reachable from config.txt, as an ownable chain —
/// the "duplicate from whatever" source: Peace's filters copy in as editable.
fn live_chain() -> Result<Vec<ChainFilter>, String> {
    let s = eq_state()?;
    Ok(s.filters
        .iter()
        .map(|f| ChainFilter {
            enabled: f.enabled,
            kind: FilterKind::from_code(f.kind).unwrap_or(FilterKind::Peaking),
            fc_hz: f.fc_hz,
            gain_db: f.gain_db,
            q: f.q,
        })
        .collect())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PresetsState {
    presets: Vec<String>,
    active: Option<String>,
}

#[tauri::command]
fn presets_state() -> Result<PresetsState, String> {
    Ok(PresetsState {
        presets: store()?.list(),
        active: active_preset(),
    })
}

/// Switch the active preset (None = flat: empty fletcher.txt).
#[tauri::command]
fn preset_switch(name: Option<String>) -> Result<EqState, String> {
    match &name {
        Some(n) => {
            let preset = store()?
                .load(n)
                .ok_or_else(|| format!("preset {n:?} not found"))?;
            activate_chain(&preset.filters)?;
        }
        None => activate_chain(&[])?,
    }
    set_active_preset(name.as_deref());
    set_ab_side("a");
    eq_state()
}

/// Create a preset — `from_live` seeds it with everything currently audible
/// (including filters owned by other tools); otherwise it starts empty.
/// The new preset becomes active.
#[tauri::command]
fn preset_create(name: String, from_live: bool) -> Result<EqState, String> {
    let name = sanitize_name(&name).ok_or("invalid preset name")?;
    let st = store()?;
    if st.exists(&name) {
        return Err(format!("a preset named {name:?} already exists"));
    }
    let chain = if from_live { live_chain()? } else { Vec::new() };
    let specs = specs_of(&chain);
    st.save(&name, matched_preamp(&specs), &chain)
        .map_err(|e| e.to_string())?;
    activate_chain(&chain)?;
    set_active_preset(Some(&name));
    eq_state()
}

/// Copy an external include's chain (e.g. peace.txt) into a Fletcher preset.
/// Never touches the source; does not activate the new preset.
#[tauri::command]
fn preset_copy_from_source(source: String, name: String) -> Result<PresetsState, String> {
    let name = sanitize_name(&name).ok_or("invalid preset name")?;
    if source.contains(['/', '\\', ':']) {
        return Err("invalid source".into());
    }
    let install = apo::detect().map_err(|e| e.to_string())?;
    let text = std::fs::read_to_string(install.config_path.join(&source))
        .map_err(|e| format!("cannot read {source}: {e}"))?;
    let doc = ConfigDoc::parse(&text);
    let mut preamp = 0.0;
    let mut chain = Vec::new();
    for d in doc.directives() {
        match d {
            Parsed::Preamp { db } => preamp += db,
            Parsed::Filter {
                enabled,
                kind,
                fc_hz,
                gain_db,
                q,
                ..
            } => chain.push(ChainFilter {
                enabled: *enabled,
                kind: *kind,
                fc_hz: fc_hz.unwrap_or(1000.0),
                gain_db: gain_db.unwrap_or(0.0),
                q: q.unwrap_or(std::f64::consts::FRAC_1_SQRT_2),
            }),
            _ => {}
        }
    }
    // Peace empties peace.txt when its EQ is toggled off, but keeps the live
    // chain in its own format — fall back to that so copy works either way.
    if chain.is_empty() && source.eq_ignore_ascii_case("peace.txt") {
        if let Ok(peace_text) =
            std::fs::read_to_string(install.config_path.join("Last Configuration.peace"))
        {
            if let Some(import) = fletcher_core::peace::parse_peace(&peace_text) {
                preamp = import.preamp_db;
                chain = import.filters;
            }
        }
    }
    if chain.is_empty() {
        return Err(format!(
            "{source} has no filters right now — turn its EQ on in the other tool first, then copy"
        ));
    }
    let st = store()?;
    if st.exists(&name) {
        return Err(format!("a preset named {name:?} already exists"));
    }
    st.save(&name, preamp, &chain).map_err(|e| e.to_string())?;
    presets_state()
}

#[tauri::command]
fn preset_duplicate(from: String, to: String) -> Result<PresetsState, String> {
    let to = sanitize_name(&to).ok_or("invalid preset name")?;
    let st = store()?;
    if st.exists(&to) {
        return Err(format!("a preset named {to:?} already exists"));
    }
    let p = st
        .load(&from)
        .ok_or_else(|| format!("preset {from:?} not found"))?;
    st.save(&to, p.preamp_db, &p.filters)
        .map_err(|e| e.to_string())?;
    presets_state()
}

/// The active preset's chain (empty when no preset is active).
fn active_chain() -> Vec<ChainFilter> {
    active_preset()
        .and_then(|n| store().ok()?.load(&n))
        .map(|p| p.filters)
        .unwrap_or_default()
}

fn specs_of(chain: &[ChainFilter]) -> Vec<FilterSpec> {
    chain
        .iter()
        .filter(|f| f.enabled)
        .map(|f| FilterSpec {
            kind: f.kind,
            fc_hz: f.fc_hz,
            gain_db: f.gain_db,
            q: f.q,
        })
        .collect()
}

/// The preamp that lands a chain at the global reference loudness (its mean
/// response over a log grid ≈ pink-noise weighting hits `reference_db`),
/// tightened to the clip-safe preamp when the reference can't be reached
/// without exceeding 0 dB peak (TB-06). Everything — flat, every preset,
/// mid-edit chains — normalizes to the same reference (Q-06/ADR-0003).
fn matched_preamp(specs: &[FilterSpec]) -> f64 {
    let clip_safe = auto_preamp_db(specs, FS);
    let reference = reference_db().min(0.0);
    if specs.is_empty() {
        return (reference * 10.0).round() / 10.0;
    }
    let freqs = log_freqs(20.0, 20000.0, 200);
    let resp = chain_response_db(specs, 0.0, FS, &freqs);
    let mean = resp.iter().sum::<f64>() / resp.len() as f64;
    let to_reference = reference - mean;
    ((to_reference.min(clip_safe) * 10.0).round() / 10.0).clamp(-30.0, 0.0)
}

/// Write fletcher.txt for the given side: A = the active chain; B = flat,
/// level-matched to the chain's average loudness.
fn apply_side(side: &str) -> Result<(), String> {
    let chain = active_chain();
    match side {
        "b" => {
            let install = apo::detect().map_err(|e| e.to_string())?;
            fsx::write_atomic(
                &install.config_path.join("fletcher.txt"),
                &render_fletcher_file(matched_preamp(&[]), &[]),
            )
            .map_err(|e| e.to_string())
        }
        _ => activate_chain(&chain),
    }
}

// ---------------- AutoEQ: fetch-on-demand headphone presets (ADR-0008) ----------------

const AUTOEQ_BASE: &str = "https://raw.githubusercontent.com/jaakkopasanen/AutoEq/master/results";
const INDEX_TTL_SECS: u64 = 7 * 24 * 3600;

fn autoeq_dir() -> PathBuf {
    data_dir().join("autoeq")
}

fn http_get(url: &str) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("Fletcher-EQ (github.com/andrewsalinas09/Fletcher)")
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(url)
        .send()
        .map_err(|e| format!("network error: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("fetch failed ({}) for {url}", resp.status()));
    }
    resp.text().map_err(|e| e.to_string())
}

/// The AutoEq results index, cached; stale-if-error (ADR-0008: cache is
/// mandatory, offline degrades to last known).
fn autoeq_index() -> Result<String, String> {
    let path = autoeq_dir().join("index.md");
    let fresh = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age.as_secs() < INDEX_TTL_SECS);
    if fresh {
        if let Ok(text) = std::fs::read_to_string(&path) {
            return Ok(text);
        }
    }
    match http_get(&format!("{AUTOEQ_BASE}/INDEX.md")) {
        Ok(text) => {
            let _ = std::fs::create_dir_all(autoeq_dir());
            let _ = fsx::write_atomic(&path, &text);
            Ok(text)
        }
        Err(e) => std::fs::read_to_string(&path)
            .map_err(|_| format!("{e} — and no cached index yet; connect once to fetch it")),
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct AutoeqEntry {
    name: String,
    /// URL-encoded directory path relative to results/, e.g. "oratory1990/over-ear/Sennheiser%20HD%20650"
    path: String,
    note: String,
}

fn parse_autoeq_index(text: &str) -> Vec<AutoeqEntry> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix("- [")?;
            let (name, rest) = rest.split_once("](")?;
            let (link, tail) = rest.split_once(')')?;
            let path = link.strip_prefix("./")?.to_string();
            Some(AutoeqEntry {
                name: name.to_string(),
                path,
                note: tail.trim().to_string(),
            })
        })
        .collect()
}

fn autoeq_search_sync(query: &str) -> Result<Vec<AutoeqEntry>, String> {
    let q = query.trim().to_lowercase();
    if q.len() < 2 {
        return Ok(Vec::new());
    }
    let tokens: Vec<&str> = q.split_whitespace().collect();
    let index = autoeq_index()?;
    Ok(parse_autoeq_index(&index)
        .into_iter()
        .filter(|e| {
            let name = e.name.to_lowercase();
            tokens.iter().all(|t| name.contains(t))
        })
        .take(40)
        .collect())
}

#[tauri::command]
async fn autoeq_search(query: String) -> Result<Vec<AutoeqEntry>, String> {
    tauri::async_runtime::spawn_blocking(move || autoeq_search_sync(&query))
        .await
        .map_err(|e| e.to_string())?
}

fn autoeq_import_sync(name: &str, path: &str) -> Result<(), String> {
    if path.contains("..") || name.contains("..") {
        return Err("invalid entry".into());
    }
    let encoded_name = name
        .replace('%', "%25")
        .replace(' ', "%20")
        .replace('#', "%23");
    let url = format!("{AUTOEQ_BASE}/{path}/{encoded_name}%20ParametricEQ.txt");

    // Cache per preset file, stale-if-error.
    let cache = autoeq_dir().join("files").join(format!(
        "{}.txt",
        sanitize_name(name).unwrap_or_else(|| "preset".into())
    ));
    let text = match http_get(&url) {
        Ok(t) => {
            let _ = std::fs::create_dir_all(cache.parent().unwrap());
            let _ = fsx::write_atomic(&cache, &t);
            t
        }
        Err(e) => std::fs::read_to_string(&cache).map_err(|_| e)?,
    };

    let doc = ConfigDoc::parse(&text);
    let mut preamp = 0.0;
    let mut chain = Vec::new();
    for d in doc.directives() {
        match d {
            Parsed::Preamp { db } => preamp += db,
            Parsed::Filter {
                enabled,
                kind,
                fc_hz,
                gain_db,
                q,
                ..
            } => chain.push(ChainFilter {
                enabled: *enabled,
                kind: *kind,
                fc_hz: fc_hz.unwrap_or(1000.0),
                gain_db: gain_db.unwrap_or(0.0),
                q: q.unwrap_or(std::f64::consts::FRAC_1_SQRT_2),
            }),
            _ => {}
        }
    }
    if chain.is_empty() {
        return Err("that AutoEQ entry contained no parametric filters".into());
    }

    let st = store()?;
    let base = sanitize_name(name).ok_or("unusable preset name")?;
    let mut preset_name = base.clone();
    let mut n = 2;
    while st.exists(&preset_name) {
        preset_name = format!("{base} {n}");
        n += 1;
    }
    st.save(&preset_name, preamp, &chain)
        .map_err(|e| e.to_string())?;
    activate_chain(&chain)?;
    set_active_preset(Some(&preset_name));
    set_ab_side("a");
    Ok(())
}

#[tauri::command]
async fn autoeq_import(name: String, path: String) -> Result<EqState, String> {
    tauri::async_runtime::spawn_blocking(move || autoeq_import_sync(&name, &path))
        .await
        .map_err(|e| e.to_string())??;
    eq_state()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PastedFilter {
    enabled: bool,
    kind: &'static str,
    fc_hz: f64,
    gain_db: f64,
    q: f64,
}

/// Parse clipboard text as APO filter lines (one or many — pasting a whole
/// preset's worth of lines works). Non-filter lines are ignored.
#[tauri::command]
fn parse_filters(text: String) -> Vec<PastedFilter> {
    ConfigDoc::parse(&text)
        .directives()
        .filter_map(|d| match d {
            Parsed::Filter {
                enabled,
                kind,
                fc_hz,
                gain_db,
                q,
                ..
            } => Some(PastedFilter {
                enabled: *enabled,
                kind: kind.code(),
                fc_hz: fc_hz.unwrap_or(1000.0),
                gain_db: gain_db.unwrap_or(0.0),
                q: q.unwrap_or(std::f64::consts::FRAC_1_SQRT_2),
            }),
            _ => None,
        })
        .collect()
}

// ---------------- the history inspector's engine access (Q-24) ----------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ChainCurve {
    response_db: Vec<f64>,
    matched_preamp_db: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ChainCurves {
    freqs: Vec<f64>,
    curves: Vec<ChainCurve>,
}

/// Response curves for arbitrary (non-live) chains, batched — one call serves a
/// whole history tree. The audible, level-matched difference between chains i
/// and j is (responseDb_i + matchedPreampDb_i) − (responseDb_j + matchedPreampDb_j).
#[tauri::command]
fn chain_curves(chains: Vec<Vec<FilterIn>>) -> Result<ChainCurves, String> {
    let freqs = log_freqs(20.0, 20000.0, CURVE_POINTS);
    let curves = chains
        .iter()
        .map(|filters| {
            let specs = specs_of(&chain_of(filters)?);
            Ok(ChainCurve {
                response_db: chain_response_db(&specs, 0.0, FS, &freqs),
                matched_preamp_db: matched_preamp(&specs),
            })
        })
        .collect::<Result<_, String>>()?;
    Ok(ChainCurves { freqs, curves })
}

/// Level-matched, non-destructive audition of an arbitrary chain (the history
/// inspector's "listen"): writes fletcher.txt only — the active preset and the
/// A/B side are untouched, so the preview leaves no trace on disk state.
/// Refused mid-ABX: an outside write would break the blinding.
#[tauri::command]
fn preview_chain(filters: Vec<FilterIn>) -> Result<(), String> {
    if ABX.lock().unwrap().is_some() {
        return Err("a blind test is running — finish or cancel it first".into());
    }
    activate_chain(&chain_of(&filters)?)
}

/// Promote an arbitrary chain (a history node) into a preset. Auto-suffixes on
/// name collision; does not activate. Returns the name actually used.
#[tauri::command]
fn preset_create_from_chain(name: String, filters: Vec<FilterIn>) -> Result<String, String> {
    let base = sanitize_name(&name).ok_or("invalid preset name")?;
    let chain = chain_of(&filters)?;
    if chain.is_empty() {
        return Err("this node has no filters to save".into());
    }
    let st = store()?;
    let mut name = base.clone();
    let mut n = 2;
    while st.exists(&name) {
        name = format!("{base} {n}");
        n += 1;
    }
    st.save(&name, matched_preamp(&specs_of(&chain)), &chain)
        .map_err(|e| e.to_string())?;
    Ok(name)
}

// ---------------- history persistence (Q-17: trees survive restarts) ----------------

fn history_dir() -> PathBuf {
    data_dir().join("history")
}

#[tauri::command]
fn history_save(preset: String, data: String) -> Result<(), String> {
    let name = sanitize_name(&preset).ok_or("bad preset name")?;
    std::fs::create_dir_all(history_dir()).map_err(|e| e.to_string())?;
    fsx::write_atomic(&history_dir().join(format!("{name}.json")), &data).map_err(|e| e.to_string())
}

#[tauri::command]
fn history_load(preset: String) -> Option<String> {
    let name = sanitize_name(&preset)?;
    std::fs::read_to_string(history_dir().join(format!("{name}.json"))).ok()
}

/// Write a history file to a user-chosen path (export via save dialog).
#[tauri::command]
fn history_export(path: String, data: String) -> Result<(), String> {
    fsx::write_atomic(std::path::Path::new(&path), &data).map_err(|e| e.to_string())
}

#[tauri::command]
fn history_import(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path).map_err(|e| e.to_string())
}

#[tauri::command]
fn preset_rename(from: String, to: String) -> Result<PresetsState, String> {
    let to = sanitize_name(&to).ok_or("invalid preset name")?;
    let st = store()?;
    if !st.exists(&from) {
        return Err(format!("preset {from:?} not found"));
    }
    if st.exists(&to) {
        return Err(format!("a preset named {to:?} already exists"));
    }
    let p = st.load(&from).ok_or("could not read preset")?;
    st.save(&to, p.preamp_db, &p.filters)
        .map_err(|e| e.to_string())?;
    st.delete(&from).map_err(|e| e.to_string())?;
    if active_preset().as_deref() == Some(from.as_str()) {
        set_active_preset(Some(&to));
    }
    presets_state()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceDto {
    id: String,
    name: String,
    is_default: bool,
}

#[tauri::command]
fn devices_list() -> Vec<DeviceDto> {
    devices::list_render_devices()
        .into_iter()
        .map(|d| DeviceDto {
            id: d.id,
            name: d.name,
            is_default: d.is_default,
        })
        .collect()
}

#[tauri::command]
fn device_set_default(id: String) -> Result<EqState, String> {
    devices::set_default_render_device(&id)?;
    eq_state()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AbInfo {
    side: String,
    match_db: f64,
}

#[tauri::command]
fn ab_info() -> AbInfo {
    AbInfo {
        side: ab_side(),
        match_db: matched_preamp(&[]),
    }
}

#[tauri::command]
fn ab_set(side: String) -> Result<EqState, String> {
    let side = if side == "b" { "b" } else { "a" };
    apply_side(side)?;
    set_ab_side(side);
    eq_state()
}

/// Flip A/B (hotkey + tray path); emits `ab-changed` with the new side.
/// During an ABX session the hotkey cycles the audition target instead.
fn ab_flip(app: &tauri::AppHandle) {
    use tauri::Emitter;
    {
        let mut guard = ABX.lock().unwrap();
        if let Some(session) = guard.as_mut() {
            let next = match session.audition.as_str() {
                "a" => "b",
                "b" => "x",
                _ => "a",
            };
            if abx_apply_audition(session, next).is_ok() {
                let _ = app.emit("abx-audition", next);
            }
            return;
        }
    }
    let next = if ab_side() == "a" { "b" } else { "a" };
    if apply_side(next).is_ok() {
        set_ab_side(next);
        let _ = app.emit("ab-changed", next);
    }
}

// ---------------- ABX: the trial room engine ----------------
//
// The session lives server-side only; X assignments never reach the UI while
// the session runs (that's the blinding). Everything is journaled and the
// finished session persists with full labels for replay (Q-05).

struct AbxSession {
    id: String,
    a_name: String,
    b_name: String,
    /// The two competing chains, captured at start. Classic mode is the active
    /// preset's chain vs an empty (flat) chain; node-vs-node passes both
    /// explicitly. Every audition writes through the level-matched path, so
    /// both sides land at the reference loudness by construction.
    a: Vec<ChainFilter>,
    b: Vec<ChainFilter>,
    planned: usize,
    assignments: Vec<bool>, // per trial: X is A
    answers: Vec<bool>,     // per answered trial: user said "X is A"
    stats_viewed: Vec<usize>,
    audition: String, // "a" | "b" | "x"
    started_ms: u64,
}

static ABX: std::sync::Mutex<Option<AbxSession>> = std::sync::Mutex::new(None);

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Write the config for what the listener should hear right now.
fn abx_apply_audition(session: &mut AbxSession, target: &str) -> Result<(), String> {
    let trial = session.answers.len();
    let hear_a = match target {
        "a" => true,
        "b" => false,
        _ => *session.assignments.get(trial).unwrap_or(&true),
    };
    activate_chain(if hear_a { &session.a } else { &session.b })?;
    session.audition = target.to_string();
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AbxState {
    active: bool,
    a_name: String,
    b_name: String,
    planned: usize,
    answered: usize,
    audition: String,
    /// Correct count so far — present only after an explicit reveal (TB-24).
    running_correct: Option<usize>,
}

fn abx_state_of(session: &AbxSession, revealed: bool) -> AbxState {
    AbxState {
        active: true,
        a_name: session.a_name.clone(),
        b_name: session.b_name.clone(),
        planned: session.planned,
        answered: session.answers.len(),
        audition: session.audition.clone(),
        running_correct: if revealed {
            Some(abx_correct(session))
        } else {
            None
        },
    }
}

fn abx_correct(session: &AbxSession) -> usize {
    session
        .answers
        .iter()
        .zip(&session.assignments)
        .filter(|(ans, actual)| ans == actual)
        .count()
}

/// Start a session. With no chains: classic mode — the active preset vs flat.
/// With both `a` and `b`: any two chains (e.g. two history nodes); level
/// matching between them comes from the shared write path.
#[tauri::command]
fn abx_start(
    trials: usize,
    a: Option<Vec<FilterIn>>,
    b: Option<Vec<FilterIn>>,
    a_name: Option<String>,
    b_name: Option<String>,
) -> Result<AbxState, String> {
    let (chain_a, chain_b, name_a, name_b) = match (a, b) {
        (Some(a), Some(b)) => (
            chain_of(&a)?,
            chain_of(&b)?,
            a_name.unwrap_or_else(|| "A".into()),
            b_name.unwrap_or_else(|| "B".into()),
        ),
        (None, None) => {
            let chain = active_chain();
            if chain.iter().filter(|f| f.enabled).count() == 0 {
                return Err("activate a preset with at least one enabled filter first — A and B would sound identical".into());
            }
            (
                chain,
                Vec::new(),
                active_preset().unwrap_or_else(|| "Fletcher chain".into()),
                "Flat".into(),
            )
        }
        _ => return Err("provide both chains or neither".into()),
    };
    // Two chains that level-match to the same response leave nothing to test.
    // Comparing responses, not filter lists, catches differently-written but
    // audibly identical chains.
    {
        let freqs = log_freqs(20.0, 20000.0, CURVE_POINTS);
        let (sa, sb) = (specs_of(&chain_a), specs_of(&chain_b));
        let ra = chain_response_db(&sa, matched_preamp(&sa), FS, &freqs);
        let rb = chain_response_db(&sb, matched_preamp(&sb), FS, &freqs);
        if ra.iter().zip(&rb).all(|(x, y)| (x - y).abs() < 0.05) {
            return Err(
                "A and B level-match to the same response — there is no difference to test".into(),
            );
        }
    }
    let trials = trials.clamp(4, 100);
    let mut rng = fletcher_core::stats::Xorshift::new(now_ms() | 1);
    let mut session = AbxSession {
        id: format!("abx-{}", now_ms()),
        a_name: name_a,
        b_name: name_b,
        a: chain_a,
        b: chain_b,
        planned: trials,
        assignments: (0..trials).map(|_| rng.next_bool()).collect(),
        answers: Vec::new(),
        stats_viewed: Vec::new(),
        audition: "a".into(),
        started_ms: now_ms(),
    };
    abx_apply_audition(&mut session, "a")?;
    let state = abx_state_of(&session, false);
    *ABX.lock().unwrap() = Some(session);
    Ok(state)
}

#[tauri::command]
fn abx_audition(target: String) -> Result<AbxState, String> {
    let mut guard = ABX.lock().unwrap();
    let session = guard.as_mut().ok_or("no session running")?;
    let target = match target.as_str() {
        "a" | "b" | "x" => target,
        _ => return Err("unknown target".into()),
    };
    abx_apply_audition(session, &target)?;
    Ok(abx_state_of(session, false))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AbxTrialLog {
    x_was_a: bool,
    answered_a: bool,
    correct: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AbxResult {
    id: String,
    a_name: String,
    b_name: String,
    /// Full provenance: what A and B actually were, and the reference both
    /// were matched to — a session replays meaningfully years later.
    a_chain: Vec<PastedFilter>,
    b_chain: Vec<PastedFilter>,
    reference_db: f64,
    trials: usize,
    correct: usize,
    p_value: f64,
    stats_viewed: Vec<usize>,
    log: Vec<AbxTrialLog>,
    started_ms: u64,
}

fn dto_chain(chain: &[ChainFilter]) -> Vec<PastedFilter> {
    chain
        .iter()
        .map(|f| PastedFilter {
            enabled: f.enabled,
            kind: f.kind.code(),
            fc_hz: f.fc_hz,
            gain_db: f.gain_db,
            q: f.q,
        })
        .collect()
}

fn abx_result_of(session: &AbxSession) -> AbxResult {
    let correct = abx_correct(session);
    AbxResult {
        id: session.id.clone(),
        a_name: session.a_name.clone(),
        b_name: session.b_name.clone(),
        a_chain: dto_chain(&session.a),
        b_chain: dto_chain(&session.b),
        reference_db: reference_db(),
        trials: session.answers.len(),
        correct,
        p_value: fletcher_core::stats::binomial_p_one_sided(
            correct as u32,
            session.answers.len() as u32,
        ),
        stats_viewed: session.stats_viewed.clone(),
        log: session
            .answers
            .iter()
            .zip(&session.assignments)
            .map(|(ans, actual)| AbxTrialLog {
                x_was_a: *actual,
                answered_a: *ans,
                correct: ans == actual,
            })
            .collect(),
        started_ms: session.started_ms,
    }
}

fn sessions_dir() -> PathBuf {
    data_dir().join("sessions")
}

/// Vote for the current trial. Returns the final result on the last trial.
#[tauri::command]
fn abx_vote(x_is_a: bool) -> Result<serde_json::Value, String> {
    let mut guard = ABX.lock().unwrap();
    let session = guard.as_mut().ok_or("no session running")?;
    if session.answers.len() >= session.planned {
        return Err("session already complete".into());
    }
    session.answers.push(x_is_a);

    if session.answers.len() == session.planned {
        let result = abx_result_of(session);
        let _ = std::fs::create_dir_all(sessions_dir());
        let json = serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?;
        let _ = fsx::write_atomic(&sessions_dir().join(format!("{}.json", result.id)), &json);
        let _ = apply_side("a");
        set_ab_side("a");
        *guard = None;
        Ok(
            serde_json::json!({ "done": true, "result": serde_json::from_str::<serde_json::Value>(&json).unwrap() }),
        )
    } else {
        // Next trial opens auditioning X (its fresh mystery assignment).
        abx_apply_audition(session, "x")?;
        Ok(
            serde_json::json!({ "done": false, "state": serde_json::to_value(abx_state_of(session, false)).unwrap() }),
        )
    }
}

/// Reveal the running score mid-session — recorded in provenance (TB-24).
#[tauri::command]
fn abx_reveal() -> Result<AbxState, String> {
    let mut guard = ABX.lock().unwrap();
    let session = guard.as_mut().ok_or("no session running")?;
    let at = session.answers.len();
    if !session.stats_viewed.contains(&at) {
        session.stats_viewed.push(at);
    }
    Ok(abx_state_of(session, true))
}

#[tauri::command]
fn abx_cancel() -> Result<(), String> {
    *ABX.lock().unwrap() = None;
    let _ = apply_side("a");
    set_ab_side("a");
    Ok(())
}

/// Past sessions, newest first.
#[tauri::command]
fn abx_sessions() -> Vec<serde_json::Value> {
    let mut out: Vec<serde_json::Value> = std::fs::read_dir(sessions_dir())
        .map(|rd| {
            rd.filter_map(|e| {
                let text = std::fs::read_to_string(e.ok()?.path()).ok()?;
                serde_json::from_str(&text).ok()
            })
            .collect()
        })
        .unwrap_or_default();
    out.sort_by_key(|v| {
        std::cmp::Reverse(v.get("startedMs").and_then(|m| m.as_u64()).unwrap_or(0))
    });
    out
}

#[tauri::command]
fn preset_delete(name: String) -> Result<EqState, String> {
    store()?.delete(&name).map_err(|e| e.to_string())?;
    if active_preset().as_deref() == Some(name.as_str()) {
        set_active_preset(None);
        activate_chain(&[])?;
    }
    eq_state()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ApoStatus {
    install_path: String,
    config_path: String,
    files: Vec<ConfigFile>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfigFile {
    name: String,
    lines: Vec<LineDto>,
}

// rename_all covers the variant names (the "kind" tag); rename_all_fields is
// needed separately for the fields inside each variant.
#[derive(Serialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
#[serde(tag = "kind")]
enum LineDto {
    Preamp {
        text: String,
        db: f64,
    },
    Filter {
        text: String,
        index: Option<u32>,
        enabled: bool,
        filter_type: &'static str,
        fc_hz: Option<f64>,
        gain_db: Option<f64>,
        q: Option<f64>,
    },
    Include {
        text: String,
        path: String,
    },
    Device {
        text: String,
        pattern: String,
    },
    Channel {
        text: String,
        spec: String,
    },
    Comment {
        text: String,
    },
    Blank {},
    Unknown {
        text: String,
    },
}

fn line_text(raw: &str) -> String {
    raw.trim_end_matches(['\n', '\r']).to_string()
}

fn to_dto(doc: &ConfigDoc) -> Vec<LineDto> {
    doc.lines
        .iter()
        .map(|line| {
            let text = line_text(&line.raw);
            match &line.parsed {
                Parsed::Preamp { db } => LineDto::Preamp { text, db: *db },
                Parsed::Filter {
                    index,
                    enabled,
                    kind,
                    fc_hz,
                    gain_db,
                    q,
                } => LineDto::Filter {
                    text,
                    index: *index,
                    enabled: *enabled,
                    filter_type: kind.code(),
                    fc_hz: *fc_hz,
                    gain_db: *gain_db,
                    q: *q,
                },
                Parsed::Include { path } => LineDto::Include {
                    text,
                    path: path.clone(),
                },
                Parsed::Device { pattern } => LineDto::Device {
                    text,
                    pattern: pattern.clone(),
                },
                Parsed::Channel { spec } => LineDto::Channel {
                    text,
                    spec: spec.clone(),
                },
                Parsed::Comment => LineDto::Comment { text },
                Parsed::Blank => LineDto::Blank {},
                Parsed::Unknown => LineDto::Unknown { text },
            }
        })
        .collect()
}

/// Read the live APO config: config.txt plus every file it includes (one level).
#[tauri::command]
fn apo_status() -> Result<ApoStatus, String> {
    let install = apo::detect().map_err(|e| e.to_string())?;
    let mut files = Vec::new();

    let root_path = install.config_path.join("config.txt");
    let root_text = std::fs::read_to_string(&root_path)
        .map_err(|e| format!("cannot read {}: {e}", root_path.display()))?;
    let root = ConfigDoc::parse(&root_text);

    let includes: Vec<String> = root
        .directives()
        .filter_map(|d| match d {
            Parsed::Include { path } => Some(path.clone()),
            _ => None,
        })
        .collect();

    files.push(ConfigFile {
        name: "config.txt".into(),
        lines: to_dto(&root),
    });

    for inc in includes {
        // Include paths resolve relative to the including file's directory.
        let path = install.config_path.join(&inc);
        let lines = match std::fs::read_to_string(&path) {
            Ok(text) => to_dto(&ConfigDoc::parse(&text)),
            Err(e) => vec![LineDto::Unknown {
                text: format!("<unreadable: {e}>"),
            }],
        };
        files.push(ConfigFile { name: inc, lines });
    }

    Ok(ApoStatus {
        install_path: install.install_path.display().to_string(),
        config_path: install.config_path.display().to_string(),
        files,
    })
}

/// Watch the APO config dir and push a `apo-config-changed` event to the UI
/// on every relevant write — the same file-watching APO itself relies on, so
/// external edits (Peace, hand edits) reflect instantly without polling.
fn spawn_config_watcher(handle: tauri::AppHandle) {
    use notify::{RecursiveMode, Watcher};
    use tauri::Emitter;

    let Ok(install) = apo::detect() else { return };
    std::thread::spawn(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        let Ok(mut watcher) = notify::recommended_watcher(tx) else {
            eprintln!("fletcher: config watcher unavailable");
            return;
        };
        if watcher
            .watch(&install.config_path, RecursiveMode::NonRecursive)
            .is_err()
        {
            eprintln!("fletcher: cannot watch {}", install.config_path.display());
            return;
        }
        let relevant = |ev: &notify::Event| {
            ev.paths.iter().any(|p| {
                p.extension().is_some_and(|e| e == "txt")
                    && p.extension().is_none_or(|e| e != "fletcher-tmp")
            })
        };
        while let Ok(ev) = rx.recv() {
            let Ok(ev) = ev else { continue };
            if !relevant(&ev) {
                continue;
            }
            // Debounce bursts (editors and Peace write several events per save).
            std::thread::sleep(std::time::Duration::from_millis(150));
            while rx.try_recv().is_ok() {}
            let _ = handle.emit("apo-config-changed", ());
        }
    });
}

fn setup_tray(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::TrayIconBuilder;
    use tauri::Manager;

    let show = MenuItem::with_id(app, "show", "Show Fletcher", true, None::<&str>)?;
    let flip = MenuItem::with_id(app, "flip", "Flip A/B\tCtrl+Shift+A", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Fletcher", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &flip, &quit])?;

    let mut tray = TrayIconBuilder::new()
        .menu(&menu)
        .tooltip("Fletcher — honest EQ")
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => {
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.show();
                    let _ = win.unminimize();
                    let _ = win.set_focus();
                }
            }
            "flip" => ab_flip(app),
            "quit" => app.exit(0),
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

/// Undo/redo shortcuts for the history pop-out, active only while it has
/// focus. Fired in Rust, forwarded to the main window's rail.
fn set_history_shortcuts(app: &tauri::AppHandle, enable: bool) {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
    let gs = app.global_shortcut();
    let pairs: [(&str, &str); 3] = [
        ("ctrl+z", "undo"),
        ("ctrl+shift+z", "redo"),
        ("ctrl+y", "redo"),
    ];
    for (sc, action) in pairs {
        if enable {
            let _ = gs.on_shortcut(sc, move |app, _shortcut, event| {
                if event.state == ShortcutState::Pressed {
                    use tauri::Emitter;
                    let _ = app.emit_to(
                        "main",
                        "hist-cmd",
                        serde_json::json!({ "type": action, "id": 0 }),
                    );
                }
            });
        } else {
            let _ = gs.unregister(sc);
        }
    }
}

fn setup_hotkey(app: &tauri::App) {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
    let result = app
        .global_shortcut()
        .on_shortcut("ctrl+shift+a", |app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                ab_flip(app);
            }
        });
    if result.is_err() {
        eprintln!("fletcher: could not register Ctrl+Shift+A (in use by another app?)");
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            spawn_config_watcher(app.handle().clone());
            let _ = setup_tray(app);
            setup_hotkey(app);
            // Make the config reflect the stored side deterministically at launch.
            let _ = apply_side(&ab_side());
            Ok(())
        })
        .on_window_event(|window, event| {
            use tauri::Manager;
            match event {
                // Only the MAIN window hides to the tray (hotkeys keep
                // working); satellite windows genuinely close.
                tauri::WindowEvent::CloseRequested { api, .. } => {
                    if window.label() == "main" {
                        api.prevent_close();
                        let _ = window.hide();
                    } else if window.label() == "history" {
                        set_history_shortcuts(window.app_handle(), false);
                    }
                }
                // The pop-out webview doesn't receive DOM keyboard events, so
                // its undo/redo keys are OS-level shortcuts registered only
                // while it is focused — they never leak into other apps.
                tauri::WindowEvent::Focused(focused) if window.label() == "history" => {
                    set_history_shortcuts(window.app_handle(), *focused);
                }
                tauri::WindowEvent::Destroyed if window.label() == "history" => {
                    set_history_shortcuts(window.app_handle(), false);
                    // A preview started from the pop-out must not outlive it.
                    use tauri::Emitter;
                    let _ = window.app_handle().emit_to(
                        "main",
                        "hist-cmd",
                        serde_json::json!({ "type": "restore", "id": 0 }),
                    );
                }
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![
            apo_status,
            eq_state,
            set_fletcher_chain,
            presets_state,
            preset_switch,
            preset_create,
            preset_copy_from_source,
            preset_duplicate,
            preset_delete,
            ab_info,
            ab_set,
            devices_list,
            device_set_default,
            abx_start,
            abx_audition,
            abx_vote,
            abx_reveal,
            abx_cancel,
            abx_sessions,
            autoeq_search,
            autoeq_import,
            preset_rename,
            parse_filters,
            chain_curves,
            preview_chain,
            preset_create_from_chain,
            history_save,
            history_load,
            history_export,
            history_import
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
