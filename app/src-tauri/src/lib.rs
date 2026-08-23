//! Fletcher's Tauri shell: thin command layer over fletcher-core (ADR-0001/0005 —
//! the UI draws, the core thinks). DTOs live here so the core stays serde-free.

use fletcher_core::config::{render_fletcher_file, ChainFilter, ConfigDoc, FilterKind, Parsed};
use fletcher_core::dsp::{auto_preamp_db, chain_response_db, log_freqs, FilterSpec};
use fletcher_core::presets::{sanitize_name, PresetStore};
use fletcher_core::{apo, devices, dsp, fsx};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

mod analysis;
mod engine;
mod tools;

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

/// The honesty switch (ADR-0003): level matching is ON unless deliberately
/// disabled in Settings; off means comparisons are marked unmatched in the UI.
fn level_matching() -> bool {
    read_state()
        .get("levelMatching")
        .and_then(|v| v.as_bool())
        .unwrap_or(true)
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
    push_chain_to_track(&chain);

    // Edits persist into the active preset, so switching away and back keeps them.
    if let Some(name) = active_preset() {
        let _ = store()?.save(&name, preamp, &chain);
    }
    // Editing implies listening to the chain: land on A.
    set_ab_side("a");

    eq_state()
}

/// Rewrite fletcher.txt from a chain (with fresh auto-preamp). Also feeds a
/// running track session — the tuning loop and the engine share one truth.
fn activate_chain(chain: &[ChainFilter]) -> Result<(), String> {
    let install = apo::detect().map_err(|e| e.to_string())?;
    let specs = specs_of(chain);
    let preamp = matched_preamp(&specs);
    fsx::write_atomic(
        &install.config_path.join("fletcher.txt"),
        &render_fletcher_file(preamp, chain),
    )
    .map_err(|e| e.to_string())?;
    push_chain_to_track(chain);
    Ok(())
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

/// A preset's chain WITHOUT activating it — the Lab's contender picker
/// hands it to trial_start.
#[tauri::command]
fn preset_chain(name: String) -> Result<Vec<PastedFilter>, String> {
    let preset = store()?
        .load(&name)
        .ok_or_else(|| format!("preset {name:?} not found"))?;
    Ok(dto_chain(&preset.filters))
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
    matched_preamp_full(specs).0
}

/// (preamp, shortfall): shortfall > 0 means the clip-safe cap won the fight
/// and the chain sits that many dB ABOVE the reference — the comparison is
/// then only imperfectly matched, and the UI must say so (TB-08 v1).
fn matched_preamp_full(specs: &[FilterSpec]) -> (f64, f64) {
    let clip_safe = auto_preamp_db(specs, FS);
    if !level_matching() {
        // Honesty switch off: clip safety only, no reference normalization —
        // every comparison is then louder-vs-quieter and the UI says so.
        return ((clip_safe.clamp(-30.0, 0.0) * 10.0).round() / 10.0, 0.0);
    }
    let reference = reference_db().min(0.0);
    if specs.is_empty() {
        return ((reference * 10.0).round() / 10.0, 0.0);
    }
    let freqs = log_freqs(20.0, 20000.0, 200);
    let resp = chain_response_db(specs, 0.0, FS, &freqs);
    let mean = resp.iter().sum::<f64>() / resp.len() as f64;
    let to_reference = reference - mean;
    let shortfall = ((to_reference - clip_safe).max(0.0) * 10.0).round() / 10.0;
    (
        ((to_reference.min(clip_safe) * 10.0).round() / 10.0).clamp(-30.0, 0.0),
        shortfall,
    )
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
    if TRIAL.lock().unwrap().is_some() {
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

// ---------------- the track engine (Clip Studio, ADR-0009) ----------------

/// M1 checkpoint, kept as the permanent output-path sanity check: open the
/// device, ramp in (TB-20), play a quiet 2 s tone, release cleanly. Returns
/// the negotiated format so the user sees the exact path their audio takes.
#[tauri::command]
fn engine_test_tone(mode: String) -> Result<String, String> {
    use fletcher_core::{playback, signal};
    if TRIAL.lock().unwrap().is_some() {
        return Err("a blind test is running — finish or cancel it first".into());
    }
    let mode = if mode == "shared" {
        playback::OutputMode::Shared
    } else {
        playback::OutputMode::Exclusive
    };
    std::thread::spawn(move || {
        let stop = std::sync::atomic::AtomicBool::new(false);
        let mut src = playback::SignalSource::new(
            |fs| {
                vec![signal::Signal::new(
                    signal::SignalKind::Sine { hz: 440.0 },
                    signal::DEFAULT_AMP,
                    fs,
                    1,
                )]
            },
            Some(2.0),
        );
        let mut started: Option<playback::StreamInfo> = None;
        playback::play(mode, &mut src, 1.0, &stop, |i| started = Some(i.clone()))
            .map_err(|e| e.to_string())?;
        let i = started.ok_or("stream never started")?;
        Ok(format!(
            "{} · {} · {} bit {} @ {} Hz",
            i.device,
            match i.mode {
                playback::OutputMode::Exclusive => "exclusive (APO bypassed)",
                playback::OutputMode::Shared => "shared (APO applies)",
            },
            i.bits,
            i.sample_type,
            i.rate
        ))
    })
    .join()
    .map_err(|_| "engine thread panicked".to_string())?
}

/// The reference-leveling noise (Q-16, first half): pink noise through the
/// SHARED path — the level-matched chain applies and the user's volume
/// controls stay live, so "adjust your volume until comfortable" means
/// exactly what it says. Stopping fades out (engine guarantee).
static CAL_NOISE: std::sync::Mutex<Option<std::sync::Arc<std::sync::atomic::AtomicBool>>> =
    std::sync::Mutex::new(None);

fn stop_cal_noise() {
    if let Some(stop) = CAL_NOISE.lock().unwrap().take() {
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

#[tauri::command]
fn calibration_noise(app: tauri::AppHandle, on: bool) -> Result<(), String> {
    use std::sync::{atomic::AtomicBool, Arc};
    if TRIAL.lock().unwrap().is_some() {
        return Err("a blind test is running — finish or cancel it first".into());
    }
    let mut guard = CAL_NOISE.lock().unwrap();
    if !on {
        if let Some(stop) = guard.take() {
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        return Ok(());
    }
    if guard.is_some() {
        return Ok(());
    }
    let stop = Arc::new(AtomicBool::new(false));
    *guard = Some(stop.clone());
    drop(guard);
    std::thread::spawn(move || {
        use fletcher_core::{playback, signal};
        let mut src = playback::SignalSource::new(
            |fs| vec![signal::Signal::new(signal::SignalKind::Pink, 0.2, fs, 42)],
            None,
        );
        let result = playback::play(playback::OutputMode::Shared, &mut src, 1.0, &stop, |_| {});
        let mut guard = CAL_NOISE.lock().unwrap();
        if let Some(cur) = guard.as_ref() {
            if Arc::ptr_eq(cur, &stop) {
                *guard = None;
            }
        }
        drop(guard);
        use tauri::Emitter;
        let _ = app.emit("cal-noise-ended", result.err().map(|e| e.to_string()));
    });
    Ok(())
}

// ---------------- Clip Studio: tools, library, track sessions (M2) ----------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolsState {
    ffmpeg: Option<String>,
    ytdlp: Option<String>,
}

fn tools_state_now() -> ToolsState {
    ToolsState {
        ffmpeg: tools::find_tool(&data_dir(), "ffmpeg").map(|p| p.display().to_string()),
        ytdlp: tools::find_tool(&data_dir(), "yt-dlp").map(|p| p.display().to_string()),
    }
}

#[tauri::command]
fn tools_state() -> ToolsState {
    tools_state_now()
}

/// Download a managed tool with consent already given in the UI (ADR-0008
/// precedent: fetch on demand, never bundle). Progress goes out as events.
#[tauri::command]
async fn tools_install(app: tauri::AppHandle, which: String) -> Result<ToolsState, String> {
    let w = which.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        use tauri::Emitter;
        let wp = w.clone();
        let appc = app.clone();
        let mut last_pct: i64 = -1;
        let emit_p = move |done: u64, total: Option<u64>| {
            let pct = total.map(|t| ((done as f64 / t.max(1) as f64) * 100.0) as i64);
            if let Some(p) = pct {
                if p != last_pct {
                    last_pct = p;
                    let _ = appc.emit(
                        "tools-progress",
                        serde_json::json!({ "which": wp, "pct": p }),
                    );
                }
            }
        };
        match w.as_str() {
            "ffmpeg" => tools::install_ffmpeg(&data_dir(), emit_p).map(|_| ()),
            "yt-dlp" => tools::install_ytdlp(&data_dir(), emit_p).map(|_| ()),
            _ => Err("unknown tool".into()),
        }
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(tools_state_now())
}

fn library_db() -> Result<rusqlite::Connection, String> {
    let dir = data_dir().join("clips");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let conn = rusqlite::Connection::open(dir.join("library.db")).map_err(|e| e.to_string())?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS tracks (
            id INTEGER PRIMARY KEY,
            kind TEXT NOT NULL DEFAULT 'file',
            title TEXT NOT NULL,
            artist TEXT,
            genre TEXT,
            path TEXT,
            source_url TEXT,
            signal_params TEXT,
            duration_s REAL,
            lufs_flat REAL,
            added_ms INTEGER NOT NULL
        );",
    )
    .map_err(|e| e.to_string())?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS clips (
            id INTEGER PRIMARY KEY,
            track_id INTEGER NOT NULL,
            kind TEXT NOT NULL DEFAULT 'clip',
            name TEXT NOT NULL,
            t_in REAL NOT NULL,
            t_out REAL NOT NULL,
            f_lo REAL,
            f_hi REAL,
            note TEXT,
            created_ms INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS clip_tags (
            clip_id INTEGER NOT NULL,
            tag TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS batteries (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            note TEXT,
            created_ms INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS battery_clips (
            battery_id INTEGER NOT NULL,
            clip_id INTEGER NOT NULL,
            ord INTEGER NOT NULL,
            PRIMARY KEY (battery_id, clip_id)
        );",
    )
    .map_err(|e| e.to_string())?;
    // Migration for databases created before lufs_flat existed.
    let _ = conn.execute("ALTER TABLE tracks ADD COLUMN lufs_flat REAL", []);
    Ok(conn)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TrackRow {
    id: i64,
    kind: String,
    title: String,
    artist: Option<String>,
    genre: Option<String>,
    path: Option<String>,
    source_url: Option<String>,
    signal_params: Option<String>,
    duration_s: Option<f64>,
    added_ms: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ClipRow {
    id: i64,
    track_id: i64,
    kind: String,
    name: String,
    t_in: f64,
    t_out: f64,
    note: Option<String>,
    tags: Vec<String>,
    created_ms: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LibraryState {
    tracks: Vec<TrackRow>,
    clips: Vec<ClipRow>,
}

fn library_state_now() -> Result<LibraryState, String> {
    let conn = library_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT id, kind, title, artist, genre, path, source_url, signal_params, duration_s, added_ms
             FROM tracks ORDER BY added_ms DESC",
        )
        .map_err(|e| e.to_string())?;
    let tracks = stmt
        .query_map([], |r| {
            Ok(TrackRow {
                id: r.get(0)?,
                kind: r.get(1)?,
                title: r.get(2)?,
                artist: r.get(3)?,
                genre: r.get(4)?,
                path: r.get(5)?,
                source_url: r.get(6)?,
                signal_params: r.get(7)?,
                duration_s: r.get(8)?,
                added_ms: r.get(9)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let mut tag_map: std::collections::HashMap<i64, Vec<String>> = std::collections::HashMap::new();
    {
        let mut tag_stmt = conn
            .prepare("SELECT clip_id, tag FROM clip_tags ORDER BY tag")
            .map_err(|e| e.to_string())?;
        let rows = tag_stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?;
        for row in rows.flatten() {
            tag_map.entry(row.0).or_default().push(row.1);
        }
    }
    let mut stmt = conn
        .prepare(
            "SELECT id, track_id, kind, name, t_in, t_out, note, created_ms
             FROM clips ORDER BY track_id, t_in",
        )
        .map_err(|e| e.to_string())?;
    let clips = stmt
        .query_map([], |r| {
            Ok(ClipRow {
                id: r.get(0)?,
                track_id: r.get(1)?,
                kind: r.get(2)?,
                name: r.get(3)?,
                t_in: r.get(4)?,
                t_out: r.get(5)?,
                note: r.get(6)?,
                tags: Vec::new(),
                created_ms: r.get(7)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let clips = clips
        .into_iter()
        .map(|mut c| {
            c.tags = tag_map.remove(&c.id).unwrap_or_default();
            c
        })
        .collect();
    Ok(LibraryState { tracks, clips })
}

/// Mark the I/O region as a clip (ADR-0004: clips are user-selected, never
/// auto-extracted). Stable ids, never reused — session provenance binds to
/// clip identity (ADR-0006).
#[tauri::command]
fn clip_create(track_id: i64, t_in: f64, t_out: f64) -> Result<LibraryState, String> {
    if t_out <= t_in || t_in < 0.0 {
        return Err("the clip range needs an in point before its out point".into());
    }
    let conn = library_db()?;
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM clips WHERE track_id = ?1",
            rusqlite::params![track_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    conn.execute(
        "INSERT INTO clips (track_id, kind, name, t_in, t_out, created_ms)
         VALUES (?1, 'clip', ?2, ?3, ?4, ?5)",
        rusqlite::params![
            track_id,
            format!("Clip {}", count + 1),
            t_in,
            t_out,
            now_ms() as i64
        ],
    )
    .map_err(|e| e.to_string())?;
    library_state_now()
}

/// Rename / annotate a clip ("add too much information" — FEATURES).
#[tauri::command]
fn clip_update(
    id: i64,
    name: Option<String>,
    note: Option<String>,
) -> Result<LibraryState, String> {
    let conn = library_db()?;
    if let Some(n) = name {
        let n = n.trim();
        if !n.is_empty() {
            conn.execute(
                "UPDATE clips SET name = ?1 WHERE id = ?2",
                rusqlite::params![n, id],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    if let Some(nt) = note {
        let nt = nt.trim();
        let val: Option<&str> = if nt.is_empty() { None } else { Some(nt) };
        conn.execute(
            "UPDATE clips SET note = ?1 WHERE id = ?2",
            rusqlite::params![val, id],
        )
        .map_err(|e| e.to_string())?;
    }
    library_state_now()
}

/// Toggle a tag on a clip (band tags and free tags share one mechanism).
#[tauri::command]
fn clip_tag(id: i64, tag: String, on: bool) -> Result<LibraryState, String> {
    let tag = tag.trim().to_lowercase();
    if tag.is_empty() {
        return library_state_now();
    }
    let conn = library_db()?;
    conn.execute(
        "DELETE FROM clip_tags WHERE clip_id = ?1 AND tag = ?2",
        rusqlite::params![id, tag],
    )
    .map_err(|e| e.to_string())?;
    if on {
        conn.execute(
            "INSERT INTO clip_tags (clip_id, tag) VALUES (?1, ?2)",
            rusqlite::params![id, tag],
        )
        .map_err(|e| e.to_string())?;
    }
    library_state_now()
}

#[tauri::command]
fn clip_delete(id: i64) -> Result<LibraryState, String> {
    let conn = library_db()?;
    conn.execute("DELETE FROM clips WHERE id = ?1", rusqlite::params![id])
        .map_err(|e| e.to_string())?;
    conn.execute(
        "DELETE FROM clip_tags WHERE clip_id = ?1",
        rusqlite::params![id],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "DELETE FROM battery_clips WHERE clip_id = ?1",
        rusqlite::params![id],
    )
    .map_err(|e| e.to_string())?;
    library_state_now()
}

// ---------------- batteries: the Lab's test material (phase 3) ----------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BatteryRow {
    id: i64,
    name: String,
    note: Option<String>,
    clip_ids: Vec<i64>,
    created_ms: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BatteryState {
    batteries: Vec<BatteryRow>,
}

fn battery_state_now() -> Result<BatteryState, String> {
    let conn = library_db()?;
    let mut stmt = conn
        .prepare("SELECT id, name, note, created_ms FROM batteries ORDER BY created_ms DESC")
        .map_err(|e| e.to_string())?;
    let mut batteries = stmt
        .query_map([], |r| {
            Ok(BatteryRow {
                id: r.get(0)?,
                name: r.get(1)?,
                note: r.get(2)?,
                clip_ids: Vec::new(),
                created_ms: r.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT battery_id, clip_id FROM battery_clips ORDER BY battery_id, ord")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
        .map_err(|e| e.to_string())?;
    let mut map: std::collections::HashMap<i64, Vec<i64>> = std::collections::HashMap::new();
    for row in rows.flatten() {
        map.entry(row.0).or_default().push(row.1);
    }
    for b in &mut batteries {
        if let Some(ids) = map.remove(&b.id) {
            b.clip_ids = ids;
        }
    }
    Ok(BatteryState { batteries })
}

#[tauri::command]
fn battery_state() -> Result<BatteryState, String> {
    battery_state_now()
}

#[tauri::command]
fn battery_create(name: String, clip_ids: Option<Vec<i64>>) -> Result<BatteryState, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("a battery needs a name".into());
    }
    let conn = library_db()?;
    conn.execute(
        "INSERT INTO batteries (name, created_ms) VALUES (?1, ?2)",
        rusqlite::params![name, now_ms() as i64],
    )
    .map_err(|e| e.to_string())?;
    let id = conn.last_insert_rowid();
    drop(conn);
    if let Some(ids) = clip_ids {
        battery_set_clips_inner(id, ids)?;
    }
    battery_state_now()
}

#[tauri::command]
fn battery_update(
    id: i64,
    name: Option<String>,
    note: Option<String>,
) -> Result<BatteryState, String> {
    let conn = library_db()?;
    if let Some(n) = name {
        let n = n.trim().to_string();
        if n.is_empty() {
            return Err("a battery needs a name".into());
        }
        conn.execute(
            "UPDATE batteries SET name = ?1 WHERE id = ?2",
            rusqlite::params![n, id],
        )
        .map_err(|e| e.to_string())?;
    }
    if let Some(n) = note {
        conn.execute(
            "UPDATE batteries SET note = ?1 WHERE id = ?2",
            rusqlite::params![n, id],
        )
        .map_err(|e| e.to_string())?;
    }
    battery_state_now()
}

/// Replace a battery's clip list wholesale; vec order = play order. Missing
/// clip ids are dropped silently (they may have been deleted since).
fn battery_set_clips_inner(id: i64, clip_ids: Vec<i64>) -> Result<(), String> {
    let mut conn = library_db()?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    tx.execute(
        "DELETE FROM battery_clips WHERE battery_id = ?1",
        rusqlite::params![id],
    )
    .map_err(|e| e.to_string())?;
    let mut seen = std::collections::HashSet::new();
    let mut ord = 0i64;
    for cid in clip_ids {
        if !seen.insert(cid) {
            continue;
        }
        let exists: bool = tx
            .query_row(
                "SELECT 1 FROM clips WHERE id = ?1",
                rusqlite::params![cid],
                |_| Ok(true),
            )
            .unwrap_or(false);
        if !exists {
            continue;
        }
        tx.execute(
            "INSERT INTO battery_clips (battery_id, clip_id, ord) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, cid, ord],
        )
        .map_err(|e| e.to_string())?;
        ord += 1;
    }
    tx.commit().map_err(|e| e.to_string())
}

#[tauri::command]
fn battery_set_clips(id: i64, clip_ids: Vec<i64>) -> Result<BatteryState, String> {
    battery_set_clips_inner(id, clip_ids)?;
    battery_state_now()
}

#[tauri::command]
fn battery_delete(id: i64) -> Result<BatteryState, String> {
    let conn = library_db()?;
    conn.execute("DELETE FROM batteries WHERE id = ?1", rusqlite::params![id])
        .map_err(|e| e.to_string())?;
    conn.execute(
        "DELETE FROM battery_clips WHERE battery_id = ?1",
        rusqlite::params![id],
    )
    .map_err(|e| e.to_string())?;
    battery_state_now()
}

/// Set (or clear) the engine's loop region — how a clip solos on loop.
#[tauri::command]
fn track_loop(a_s: f64, b_s: f64, on: bool) -> Result<(), String> {
    use std::sync::atomic::Ordering::Relaxed;
    if trial_owns_track() {
        return Err("a blind trial owns the player — finish or cancel it first".into());
    }
    let guard = TRACK.lock().unwrap();
    let s = guard.as_ref().ok_or("no track playing")?;
    let rate = s.shared.rate as f64;
    s.shared.loop_a.store((a_s.max(0.0) * rate) as u64, Relaxed);
    s.shared.loop_b.store((b_s.max(0.0) * rate) as u64, Relaxed);
    s.shared.loop_on.store(on && b_s > a_s, Relaxed);
    Ok(())
}

#[tauri::command]
fn library_state() -> Result<LibraryState, String> {
    library_state_now()
}

/// Import a local audio file: referenced in place, never copied (data-dir
/// rule). Title from the filename for now; richer metadata comes with ffprobe.
#[tauri::command]
fn track_import(path: String) -> Result<LibraryState, String> {
    let p = PathBuf::from(&path);
    if !p.is_file() {
        return Err("file not found".into());
    }
    let title = p
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "track".into());
    let conn = library_db()?;
    conn.execute(
        "INSERT INTO tracks (kind, title, path, added_ms) VALUES ('file', ?1, ?2, ?3)",
        rusqlite::params![title, path, now_ms() as i64],
    )
    .map_err(|e| e.to_string())?;
    library_state_now()
}

// ---------------- the signal generator (library items, M6) ----------------

/// A generated signal's recipe — stored as `signal_params` JSON on the track
/// row. Deterministic (seed = track id), so the row IS the provenance: the
/// exact audio regenerates from the recipe forever. This JSON is the future
/// API/MCP surface (user ruling 2026-08-23) — additive changes only.
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SigSpec {
    /// white | pink | sine | sweepLog | sweepLinear | band | mix
    kind: String,
    seconds: f64,
    /// Peak level in dBFS; the generator hard-caps at −12 regardless (TB-20).
    /// For a mix the cap applies to the SUM — layers can't stack past it.
    level_db: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    hz: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    from_hz: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    to_hz: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sweep_s: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lo_hz: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hi_hz: Option<f64>,
    /// Tremolo: rate in Hz + depth 0..1 (1 = full gating). Both or neither.
    #[serde(skip_serializing_if = "Option::is_none")]
    am_hz: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    am_depth: Option<f64>,
    /// Vibrato (tonal kinds): rate in Hz + ±deviation in Hz.
    #[serde(skip_serializing_if = "Option::is_none")]
    fm_hz: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fm_dev_hz: Option<f64>,
    /// kind == "mix": the primitives to sum. One level deep — no mix in a mix.
    #[serde(skip_serializing_if = "Option::is_none")]
    layers: Option<Vec<SigSpec>>,
}

const SIG_MAX_LAYERS: usize = 16;

/// Build the signal stack a spec describes: one primitive, or a mix's layers
/// (decorrelated seeds per layer so stacked noises don't sum coherently).
/// Layers inherit the parent's duration for one-pass sweep pacing.
///
/// A mix whose worst-case sum (Σ layer peaks) exceeds the −12 dBFS cap is
/// AUTO-TRIMMED — every layer scaled by the same factor so the sum can never
/// reach the cap. Trimming beats clamping: the hard clamp distorted audibly
/// at beat crests (two −12 dB sines sum to −6). The clamp downstream stays
/// as a backstop but never engages.
fn sig_build(
    spec: &SigSpec,
    fs: f64,
    seed: u64,
) -> Result<Vec<fletcher_core::signal::Signal>, String> {
    sig_build_scaled(spec, fs, seed, 1.0)
}

fn sig_build_scaled(
    spec: &SigSpec,
    fs: f64,
    seed: u64,
    scale: f64,
) -> Result<Vec<fletcher_core::signal::Signal>, String> {
    use fletcher_core::signal::{Signal, MAX_AMP};
    if spec.kind == "mix" {
        let layers = spec.layers.as_deref().unwrap_or(&[]);
        if layers.is_empty() {
            return Err("a mix needs at least one layer".into());
        }
        if layers.len() > SIG_MAX_LAYERS {
            return Err(format!("a mix holds at most {SIG_MAX_LAYERS} layers"));
        }
        let total: f64 = layers.iter().map(|l| sig_amp(l).min(MAX_AMP)).sum();
        let trim = if total > MAX_AMP {
            MAX_AMP / total
        } else {
            1.0
        };
        let mut out = Vec::new();
        for (i, l) in layers.iter().enumerate() {
            if l.kind == "mix" {
                return Err("layers can't nest another mix (one level for now)".into());
            }
            let mut lay = l.clone();
            lay.seconds = spec.seconds;
            out.extend(sig_build_scaled(
                &lay,
                fs,
                seed.wrapping_add(0x9E37_79B9_7F4A_7C15u64.wrapping_mul(i as u64 + 1)),
                scale * trim,
            )?);
        }
        Ok(out)
    } else {
        let kind = sig_kind_of(spec)?;
        let mut s = Signal::new(kind, sig_amp(spec) * scale, fs, seed);
        if let (Some(r), Some(d)) = (spec.am_hz, spec.am_depth) {
            s = s.with_am(r, d);
        }
        if let (Some(r), Some(d)) = (spec.fm_hz, spec.fm_dev_hz) {
            s = s.with_fm(r, d);
        }
        Ok(vec![s])
    }
}

/// Full recipe validation — shared by create, preview, and the text editor.
fn sig_validate(spec: &SigSpec) -> Result<(), String> {
    if !(1.0..=600.0).contains(&spec.seconds) {
        return Err("duration must be 1–600 s".into());
    }
    sig_build(spec, 48000.0, 1).map(|_| ())
}

fn sig_kind_of(spec: &SigSpec) -> Result<fletcher_core::signal::SignalKind, String> {
    use fletcher_core::signal::SignalKind as K;
    let f = |v: Option<f64>, name: &str| -> Result<f64, String> {
        let v = v.ok_or_else(|| format!("{name} is required"))?;
        if !(10.0..=24000.0).contains(&v) {
            return Err(format!("{name} must be 10–24000 Hz"));
        }
        Ok(v)
    };
    Ok(match spec.kind.as_str() {
        "white" => K::White,
        "pink" => K::Pink,
        "sine" => K::Sine {
            hz: f(spec.hz, "frequency")?,
        },
        "sweepLog" | "sweepLinear" => {
            let from_hz = f(spec.from_hz, "from")?;
            let to_hz = f(spec.to_hz, "to")?;
            let seconds = spec.sweep_s.unwrap_or(spec.seconds).clamp(0.1, 600.0);
            if spec.kind == "sweepLog" {
                K::SweepLog {
                    from_hz,
                    to_hz,
                    seconds,
                }
            } else {
                K::SweepLinear {
                    from_hz,
                    to_hz,
                    seconds,
                }
            }
        }
        "band" => {
            let lo_hz = f(spec.lo_hz, "low edge")?;
            let hi_hz = f(spec.hi_hz, "high edge")?;
            if hi_hz <= lo_hz {
                return Err("the high edge must sit above the low edge".into());
            }
            K::BandNoise { lo_hz, hi_hz }
        }
        other => return Err(format!("unknown signal type: {other}")),
    })
}

fn sig_amp(spec: &SigSpec) -> f64 {
    10f64.powf(spec.level_db.clamp(-60.0, 0.0) / 20.0)
}

fn fmt_sig_hz(v: f64) -> String {
    if v >= 1000.0 {
        let k = v / 1000.0;
        if (k - k.round()).abs() < 1e-6 {
            format!("{k:.0} kHz")
        } else {
            format!("{k:.2} kHz")
        }
    } else {
        format!("{v:.0} Hz")
    }
}

fn sig_name(spec: &SigSpec) -> String {
    match spec.kind.as_str() {
        "white" => "White noise".into(),
        "pink" => "Pink noise".into(),
        "sine" => format!("Sine {}", fmt_sig_hz(spec.hz.unwrap_or(0.0))),
        "sweepLog" => format!(
            "Sweep {}–{} (log)",
            fmt_sig_hz(spec.from_hz.unwrap_or(0.0)),
            fmt_sig_hz(spec.to_hz.unwrap_or(0.0))
        ),
        "sweepLinear" => format!(
            "Sweep {}–{} (linear)",
            fmt_sig_hz(spec.from_hz.unwrap_or(0.0)),
            fmt_sig_hz(spec.to_hz.unwrap_or(0.0))
        ),
        "band" => format!(
            "Band noise {}–{}",
            fmt_sig_hz(spec.lo_hz.unwrap_or(0.0)),
            fmt_sig_hz(spec.hi_hz.unwrap_or(0.0))
        ),
        "mix" => {
            let names: Vec<String> = spec
                .layers
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .map(sig_name)
                .collect();
            let joined = names.join(" + ");
            if joined.len() > 48 {
                format!("Mix · {} layers", names.len())
            } else {
                joined
            }
        }
        _ => "Signal".into(),
    }
}

fn sig_title(spec: &SigSpec) -> String {
    format!("{} · {:.0} s", sig_name(spec), spec.seconds)
}

/// Render a spec into the same PCM cache a decoded file uses — everything
/// downstream (playback, waveform, spectrogram, clips) then works on signals
/// with zero further code. Specs are immutable once created, so an existing
/// cache file is always current.
fn synthesize_pcm(
    cache_dir: &std::path::Path,
    id: i64,
    rate: u32,
    spec: &SigSpec,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(cache_dir).map_err(|e| e.to_string())?;
    let out = cache_dir.join(format!("{id}-{rate}.f32"));
    if out.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        return Ok(out);
    }
    let mut gens = sig_build(spec, rate as f64, id as u64)?;
    let frames = (spec.seconds.clamp(1.0, 600.0) * rate as f64) as usize;
    // 100 ms edge fades: a generated file must never click at its ends.
    let fade = (rate as f64 * 0.1) as usize;
    let mut bytes: Vec<u8> = Vec::with_capacity(frames * 8);
    for i in 0..frames {
        // The −12 dBFS cap binds the SUM: layers can't stack past it (TB-20).
        let sum: f64 = gens.iter_mut().map(|g| g.next_sample()).sum();
        let mut s = sum.clamp(
            -fletcher_core::signal::MAX_AMP,
            fletcher_core::signal::MAX_AMP,
        );
        if i < fade {
            s *= i as f64 / fade as f64;
        }
        if frames - 1 - i < fade {
            s *= (frames - 1 - i) as f64 / fade as f64;
        }
        let b = (s as f32).to_le_bytes();
        bytes.extend_from_slice(&b);
        bytes.extend_from_slice(&b);
    }
    let tmp = out.with_extension("f32tmp");
    std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &out).map_err(|e| e.to_string())?;
    Ok(out)
}

/// The one PCM gate: every consumer — playback, waveform, sample windows,
/// spectrogram, FFT — asks here. Files decode via ffmpeg; signals synthesize
/// deterministically (and need no ffmpeg at all).
fn prepare_pcm(
    id: i64,
    rate: u32,
    on_decode: impl FnOnce(),
) -> Result<std::sync::Arc<Vec<f32>>, String> {
    let conn = library_db()?;
    let (kind, path, params): (String, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT kind, path, signal_params FROM tracks WHERE id = ?1",
            rusqlite::params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|_| "track not found".to_string())?;
    drop(conn);
    let cache = data_dir().join("cache").join("pcm");
    let pcm_path = if kind == "signal" {
        let spec: SigSpec = serde_json::from_str(&params.ok_or("signal row without params")?)
            .map_err(|e| format!("bad signal params: {e}"))?;
        synthesize_pcm(&cache, id, rate, &spec)?
    } else {
        let ffmpeg = tools::find_tool(&data_dir(), "ffmpeg")
            .ok_or("ffmpeg is not installed — use the banner in Clip Studio to set it up")?;
        let src = path.ok_or("track has no file path")?;
        engine::ensure_pcm(
            &ffmpeg,
            &cache,
            std::path::Path::new(&src),
            id,
            rate,
            on_decode,
        )?
    };
    engine::load_pcm(&pcm_path)
}

/// Add a generated signal to the library (kind='signal'): the recipe is the
/// row; PCM synthesizes on first use into the normal cache.
#[tauri::command]
fn signal_create(spec: SigSpec) -> Result<LibraryState, String> {
    sig_validate(&spec)?;
    let title = sig_title(&spec);
    let params = serde_json::to_string(&spec).map_err(|e| e.to_string())?;
    let conn = library_db()?;
    conn.execute(
        "INSERT INTO tracks (kind, title, signal_params, duration_s, added_ms)
         VALUES ('signal', ?1, ?2, ?3, ?4)",
        rusqlite::params![title, params, spec.seconds, now_ms() as i64],
    )
    .map_err(|e| e.to_string())?;
    library_state_now()
}

/// Edit a created signal in place: the recipe replaces the old one, every
/// derived artifact (PCM cache, waveform, spectrogram, memo, stored LUFS —
/// the level may have changed) is invalidated, and a session playing it is
/// stopped: it would otherwise keep playing audio the library no longer
/// describes.
#[tauri::command]
fn signal_update(id: i64, spec: SigSpec) -> Result<LibraryState, String> {
    sig_validate(&spec)?;
    let conn = library_db()?;
    let kind: String = conn
        .query_row(
            "SELECT kind FROM tracks WHERE id = ?1",
            rusqlite::params![id],
            |r| r.get(0),
        )
        .map_err(|_| "track not found".to_string())?;
    if kind != "signal" {
        return Err("only created signals can be edited".into());
    }
    let title = sig_title(&spec);
    let params = serde_json::to_string(&spec).map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE tracks SET title = ?1, signal_params = ?2, duration_s = ?3, lufs_flat = NULL
         WHERE id = ?4",
        rusqlite::params![title, params, spec.seconds, id],
    )
    .map_err(|e| e.to_string())?;
    drop(conn);
    {
        let guard = TRACK.lock().unwrap();
        if guard.as_ref().is_some_and(|s| s.track_id == id) {
            drop(guard);
            track_stop_inner();
        }
    }
    WAVES.lock().unwrap().remove(&id);
    SPECS.lock().unwrap().retain(|k, _| k.0 != id);
    {
        let mut memo = PCM_MEMO.lock().unwrap();
        if memo.as_ref().is_some_and(|(mid, _, _)| *mid == id) {
            *memo = None;
        }
    }
    let cache = data_dir().join("cache").join("pcm");
    if let Ok(rd) = std::fs::read_dir(&cache) {
        for e in rd.flatten() {
            if e.file_name()
                .to_string_lossy()
                .starts_with(&format!("{id}-"))
            {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    library_state_now()
}

/// Reveal Fletcher's data folder (media, caches, clip library) in Explorer.
#[tauri::command]
fn open_data_dir() -> Result<(), String> {
    let dir = data_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::process::Command::new("explorer")
        .arg(&dir)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Validate a recipe (the text editor's Apply): returns the auto title it
/// would get in the library.
#[tauri::command]
fn signal_validate(spec: SigSpec) -> Result<String, String> {
    sig_validate(&spec)?;
    Ok(sig_title(&spec))
}

/// Monotonic preview serial: a replaced preview's "ended" must never clobber
/// its successor's UI state (the exact bug the track sessions solved).
static PREVIEW_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Audition a spec before adding it: loops through the SHARED path (your
/// volume knob stays live) on the calibration aux stream — one aux stream
/// ever; track playback and ABX interlocks already stop it. `None` stops.
/// Returns this preview's serial; "sig-preview-ended" carries it back.
#[tauri::command]
fn signal_preview(app: tauri::AppHandle, spec: Option<SigSpec>) -> Result<u64, String> {
    use std::sync::{atomic::AtomicBool, Arc};
    if TRIAL.lock().unwrap().is_some() {
        return Err("a blind test is running — finish or cancel it first".into());
    }
    let Some(spec) = spec else {
        stop_cal_noise();
        return Ok(PREVIEW_GEN.load(std::sync::atomic::Ordering::Relaxed));
    };
    sig_validate(&spec)?;
    stop_cal_noise();
    let my_gen = PREVIEW_GEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    let stop = Arc::new(AtomicBool::new(false));
    *CAL_NOISE.lock().unwrap() = Some(stop.clone());
    std::thread::spawn(move || {
        use fletcher_core::playback;
        // Validated above — a failed rebuild yields an empty stack, which
        // just ends the stream.
        let mut src = playback::SignalSource::new(
            move |fs| sig_build(&spec, fs, 1).unwrap_or_default(),
            None,
        );
        let _ = playback::play(playback::OutputMode::Shared, &mut src, 1.0, &stop, |_| {});
        let mut guard = CAL_NOISE.lock().unwrap();
        if let Some(cur) = guard.as_ref() {
            if Arc::ptr_eq(cur, &stop) {
                *guard = None;
            }
        }
        drop(guard);
        use tauri::Emitter;
        let _ = app.emit("sig-preview-ended", my_gen);
    });
    Ok(my_gen)
}

/// Import from a URL via yt-dlp (M7): audio extracted to m4a in the managed
/// media dir, metadata from the info JSON, provenance kept in `source_url`.
/// Progress goes out as "ytdlp-progress" events (percent).
#[tauri::command]
async fn track_import_url(app: tauri::AppHandle, url: String) -> Result<LibraryState, String> {
    let url = url.trim().to_string();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("that doesn't look like a link (it should start with http)".into());
    }
    {
        let conn = library_db()?;
        let dup: Option<String> = conn
            .query_row(
                "SELECT title FROM tracks WHERE source_url = ?1",
                rusqlite::params![url],
                |r| r.get(0),
            )
            .ok();
        if let Some(t) = dup {
            return Err(format!("already in the library as \"{t}\""));
        }
    }
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        use std::io::BufRead;
        use tauri::Emitter;
        let ytdlp = tools::find_tool(&data_dir(), "yt-dlp")
            .ok_or("yt-dlp is not installed — install it from this dialog first")?;
        let ffmpeg = tools::find_tool(&data_dir(), "ffmpeg")
            .ok_or("ffmpeg is not installed — install it from this dialog first")?;
        let media = data_dir().join("media");
        std::fs::create_dir_all(&media).map_err(|e| e.to_string())?;
        let mut child = std::process::Command::new(&ytdlp)
            .args([
                "--no-playlist",
                "-f",
                "bestaudio/best",
                "-x",
                "--audio-format",
                "m4a",
                "--newline",
                // --print-json implies quiet, and the quiet progress printer
                // only emits the first and last lines (the 0→100 jump the
                // user saw). --no-quiet restores the full stream; the JSON
                // still goes to stdout, and only '{'-lines are read as JSON,
                // so the extra chatter (all '['-prefixed) is harmless. Both
                // streams are parsed — where progress lands varies by phase.
                "--progress",
                "--print-json",
                "--no-quiet",
                "--ffmpeg-location",
            ])
            .arg(&ffmpeg)
            .arg("-o")
            .arg(media.join("%(id)s.%(ext)s"))
            .arg(&url)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("could not run yt-dlp: {e}"))?;
        // "[download]  42.3% of ..." → 42.3; extraction phase → −1 (the
        // ffmpeg tail has no percentage, but silence reads as a hang).
        fn dl_pct(line: &str) -> Option<f64> {
            if line.starts_with("[ExtractAudio]") || line.starts_with("[Merger]") {
                return Some(-1.0);
            }
            line.strip_prefix("[download]")?
                .trim()
                .split('%')
                .next()?
                .trim()
                .parse::<f64>()
                .ok()
        }
        // Parse stderr line-by-line on its own thread: progress lives there in
        // quiet mode, and a drained pipe can never deadlock the child.
        let err_buf = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let stderr_handle = {
            let se = child.stderr.take();
            let buf = err_buf.clone();
            let app_err = app.clone();
            std::thread::spawn(move || {
                if let Some(se) = se {
                    for line in std::io::BufReader::new(se).lines().map_while(Result::ok) {
                        if let Some(pct) = dl_pct(&line) {
                            let _ = app_err.emit("ytdlp-progress", pct);
                        } else {
                            let mut b = buf.lock().unwrap();
                            b.push_str(&line);
                            b.push('\n');
                        }
                    }
                }
            })
        };
        let stdout = child.stdout.take().ok_or("yt-dlp gave no output stream")?;
        let mut json_line = String::new();
        for line in std::io::BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
        {
            if line.starts_with('{') {
                json_line = line;
                continue;
            }
            if let Some(pct) = dl_pct(&line) {
                let _ = app.emit("ytdlp-progress", pct);
            }
        }
        let status = child.wait().map_err(|e| e.to_string())?;
        let _ = stderr_handle.join();
        if !status.success() {
            let err = err_buf.lock().unwrap();
            let last = err
                .lines()
                .rev()
                .find(|l| l.contains("ERROR"))
                .or_else(|| err.lines().last())
                .unwrap_or("yt-dlp failed")
                .trim()
                .to_string();
            return Err(format!("download failed: {last}"));
        }
        let info: serde_json::Value =
            serde_json::from_str(&json_line).map_err(|_| "yt-dlp returned no metadata")?;
        let vid = info
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or("no id in the metadata")?;
        let title = info
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("Untitled")
            .to_string();
        let artist = info
            .get("uploader")
            .or_else(|| info.get("channel"))
            .and_then(|v| v.as_str())
            .map(String::from);
        let duration = info.get("duration").and_then(|v| v.as_f64());
        let path = media.join(format!("{vid}.m4a"));
        if !path.is_file() {
            return Err("yt-dlp finished but the audio file is missing".into());
        }
        let conn = library_db()?;
        conn.execute(
            "INSERT INTO tracks (kind, title, artist, path, source_url, duration_s, added_ms)
             VALUES ('url', ?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                title,
                artist,
                path.display().to_string(),
                url,
                duration,
                now_ms() as i64
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())??;
    library_state_now()
}

#[tauri::command]
fn track_delete(id: i64) -> Result<LibraryState, String> {
    {
        let guard = TRACK.lock().unwrap();
        if guard.as_ref().is_some_and(|s| s.track_id == id) {
            drop(guard);
            track_stop_inner();
        }
    }
    WAVES.lock().unwrap().remove(&id);
    SPECS.lock().unwrap().retain(|k, _| k.0 != id);
    {
        let mut memo = PCM_MEMO.lock().unwrap();
        if memo.as_ref().is_some_and(|(mid, _, _)| *mid == id) {
            *memo = None;
        }
    }
    let conn = library_db()?;
    conn.execute("DELETE FROM tracks WHERE id = ?1", rusqlite::params![id])
        .map_err(|e| e.to_string())?;
    conn.execute(
        "DELETE FROM clip_tags WHERE clip_id IN (SELECT id FROM clips WHERE track_id = ?1)",
        rusqlite::params![id],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "DELETE FROM clips WHERE track_id = ?1",
        rusqlite::params![id],
    )
    .map_err(|e| e.to_string())?;
    // Drop any decoded caches for this track.
    let cache = data_dir().join("cache").join("pcm");
    if let Ok(rd) = std::fs::read_dir(&cache) {
        for e in rd.flatten() {
            if e.file_name()
                .to_string_lossy()
                .starts_with(&format!("{id}-"))
            {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    library_state_now()
}

/// Clip Studio's play method (user ruling 2026-08-23): curation BYPASSES —
/// exclusive device, the track itself, level-matched toward the reference,
/// no EQ (you're studying the material, not the correction). "Through your
/// EQ" (shared path, APO applies, a regular media player) is the opt-in.
fn studio_mode() -> String {
    read_state()
        .get("studioMode")
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_else(|| "bypass".into())
}

#[tauri::command]
fn studio_state() -> serde_json::Value {
    serde_json::json!({ "mode": studio_mode() })
}

/// Takes effect on the next play.
#[tauri::command]
fn set_studio_mode(mode: String) {
    let m = if mode == "eq" { "eq" } else { "bypass" };
    write_state_field("studioMode", serde_json::json!(m));
}

/// One playback session at a time — the device is a singleton in exclusive
/// mode (ABX-static pattern).
static TRACK: std::sync::Mutex<Option<engine::TrackSession>> = std::sync::Mutex::new(None);

/// Monotonic session serial: events carry it so a stale session's "ended"
/// can never clobber the UI state of its successor.
static TRACK_SESS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Stop the current session; returns its `done` flag so a handoff can wait
/// for the fade-out + device release (streams must never overlap).
fn track_stop_inner() -> Option<std::sync::Arc<std::sync::atomic::AtomicBool>> {
    TRACK.lock().unwrap().take().map(|s| {
        s.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        s.done.clone()
    })
}

/// Live edits reach the engine's bus A (specs + fresh matched preamp). The
/// true-LUFS trim from session start is kept: recomputing per drag-tick is
/// too slow, and the config preamps already hold both buses near the
/// reference — the trim is the measured residual.
fn push_chain_to_track(chain: &[ChainFilter]) {
    use std::sync::atomic::Ordering::Relaxed;
    // A trial's chains are captured at start — live EQ edits must not
    // mutate what a blind session is playing.
    if trial_owns_track() {
        return;
    }
    let guard = TRACK.lock().unwrap();
    if let Some(s) = guard.as_ref() {
        let specs = specs_of(chain);
        let preamp_a = matched_preamp(&specs);
        {
            let mut bus = s.shared.bus.lock().unwrap();
            bus.specs = specs;
            bus.preamp_a_db = preamp_a;
        }
        s.shared.chain_gen.fetch_add(1, Relaxed);
    }
}

/// While an exclusive track session runs, A/B lives inside the engine —
/// config writes are irrelevant to what's playing. Returns true if handled.
fn track_engine_side_set(side: &str) -> bool {
    use std::sync::atomic::Ordering::Relaxed;
    let guard = TRACK.lock().unwrap();
    if let Some(s) = guard.as_ref() {
        if s.shared.bus.lock().unwrap().in_engine_eq {
            s.shared.side_b.store(side == "b", Relaxed);
            return true;
        }
    }
    false
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TrackState {
    active: bool,
    track_id: Option<i64>,
    paused: bool,
    pos_s: f64,
    duration_s: f64,
    mode: Option<&'static str>,
}

fn track_state_now() -> TrackState {
    use std::sync::atomic::Ordering::Relaxed;
    let guard = TRACK.lock().unwrap();
    match guard.as_ref() {
        Some(s) => TrackState {
            active: true,
            track_id: Some(s.track_id),
            paused: s.shared.paused.load(Relaxed),
            pos_s: s.shared.pos.load(Relaxed) as f64 / s.shared.rate as f64,
            duration_s: s.shared.total_frames as f64 / s.shared.rate as f64,
            mode: Some(match s.mode {
                fletcher_core::playback::OutputMode::Exclusive => "exclusive",
                fletcher_core::playback::OutputMode::Shared => "shared",
            }),
        },
        None => TrackState {
            active: false,
            track_id: None,
            paused: false,
            pos_s: 0.0,
            duration_s: 0.0,
            mode: None,
        },
    }
}

/// Start playing a library track through the engine, per the studio play
/// method: **bypass** (default — curation studies the track itself:
/// exclusive device, no EQ, level-matched toward the reference via stored
/// flat LUFS) or **eq** (a regular player: shared path, APO applies your
/// chain and the normal A/B). Decode is cached per (track, rate).
#[tauri::command]
async fn track_play(app: tauri::AppHandle, id: i64) -> Result<(), String> {
    use fletcher_core::playback::OutputMode;
    if TRIAL.lock().unwrap().is_some() {
        return Err("a blind test is running — finish or cancel it first".into());
    }
    stop_cal_noise();
    let prior_done = track_stop_inner();
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        use std::sync::atomic::Ordering::Relaxed;
        use tauri::Emitter;
        // Never overlap streams: wait for the previous session's fade-out
        // and device release before opening a new one.
        if let Some(done) = prior_done {
            let t0 = std::time::Instant::now();
            while !done.load(Relaxed) && t0.elapsed() < std::time::Duration::from_secs(3) {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
        let bypass = studio_mode() != "eq";
        let (mode, rate) = if bypass {
            match fletcher_core::playback::probe_rate(OutputMode::Exclusive) {
                Ok(r) => (OutputMode::Exclusive, r),
                // Device refused exclusive — degrade to the shared path and
                // say so (the chip shows "bypass unavailable").
                Err(_) => (OutputMode::Shared, 48000),
            }
        } else {
            (OutputMode::Shared, 48000)
        };
        let exclusive = mode == OutputMode::Exclusive;
        let sess = TRACK_SESS.fetch_add(1, Relaxed) + 1;
        let app_decode = app.clone();
        // Only a real ffmpeg run announces itself — cache hits and synthesized
        // signals play at once.
        let pcm = prepare_pcm(id, rate, move || {
            let _ = app_decode.emit(
                "track-state",
                serde_json::json!({ "event": "decoding", "trackId": id, "sess": sess }),
            );
        })?;
        if pcm.is_empty() {
            return Err("this file decoded to nothing — is it audio?".into());
        }
        let duration_s = (pcm.len() / 2) as f64 / rate as f64;
        if let Ok(conn) = library_db() {
            let _ = conn.execute(
                "UPDATE tracks SET duration_s = ?1 WHERE id = ?2",
                rusqlite::params![duration_s, id],
            );
        }

        // The track's flat LUFS: measured once ever, stored, then reused —
        // it's what bypass mode level-matches with, and Q-06 validation later.
        let mut lufs_flat: Option<f64> = library_db().ok().and_then(|conn| {
            conn.query_row(
                "SELECT lufs_flat FROM tracks WHERE id = ?1",
                rusqlite::params![id],
                |r| r.get::<_, Option<f64>>(0),
            )
            .ok()
            .flatten()
        });
        if lufs_flat.is_none() {
            if let Ok(l) = engine::lufs_through(&pcm, rate, &[], 0.0) {
                if l.is_finite() {
                    lufs_flat = Some(l);
                    if let Ok(conn) = library_db() {
                        let _ = conn.execute(
                            "UPDATE tracks SET lufs_flat = ?1 WHERE id = ?2",
                            rusqlite::params![l, id],
                        );
                    }
                }
            }
        }

        // Bypass level match: bring the raw track toward the reference
        // loudness (target scales with referenceDb; −16 LUFS at the −8
        // default). Attenuate-only — a positive gain could clip (TB-06).
        let gain_db = if bypass {
            let target = -16.0 + (reference_db() + 8.0);
            lufs_flat.map(|l| (target - l).min(0.0)).unwrap_or(0.0)
        } else {
            0.0
        };

        let session = engine::TrackSession::new(id, mode, rate, (pcm.len() / 2) as u64, 0);
        let (app_start, app_end, app_pos) = (app.clone(), app.clone(), app.clone());
        let stop_end = session.stop.clone();
        let (shared_pos, stop_pos, done_pos) = (
            session.shared.clone(),
            session.stop.clone(),
            session.done.clone(),
        );
        session.spawn_audio(
            pcm,
            10f64.powf(gain_db / 20.0),
            move |info| {
                let _ = app_start.emit(
                    "track-state",
                    serde_json::json!({
                        "event": "started", "trackId": id, "sess": sess, "durationS": duration_s,
                        "mode": if bypass { "bypass" } else { "eq" },
                        "exclusive": exclusive, "gainDb": gain_db,
                        "device": info.device, "rate": info.rate, "bits": info.bits,
                    }),
                );
            },
            move |err| {
                let mut guard = TRACK.lock().unwrap();
                if let Some(cur) = guard.as_ref() {
                    if std::sync::Arc::ptr_eq(&cur.stop, &stop_end) {
                        *guard = None;
                    }
                }
                drop(guard);
                let _ = app_end.emit(
                    "track-state",
                    serde_json::json!({ "event": "ended", "trackId": id, "sess": sess, "error": err }),
                );
            },
        );
        // ~10 Hz transport position for the UI; dies with the session. It
        // must watch `done` too: a NATURALLY finished track raises done but
        // never stop, and an immortal emitter here made every later session's
        // clock flash between two times (the bug the user caught with a 30 s
        // mix that ran to its end).
        std::thread::spawn(move || {
            use std::sync::atomic::Ordering::Relaxed;
            while !stop_pos.load(Relaxed) && !done_pos.load(Relaxed) {
                let _ = app_pos.emit(
                    "track-pos",
                    serde_json::json!({
                        "trackId": id,
                        "posS": shared_pos.pos.load(Relaxed) as f64 / shared_pos.rate as f64,
                        "paused": shared_pos.paused.load(Relaxed),
                    }),
                );
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        });
        *TRACK.lock().unwrap() = Some(session);
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

fn track_toggle_inner() -> bool {
    use std::sync::atomic::Ordering::Relaxed;
    let guard = TRACK.lock().unwrap();
    match guard.as_ref() {
        Some(s) => {
            let now = !s.shared.paused.load(Relaxed);
            s.shared.paused.store(now, Relaxed);
            true
        }
        None => false,
    }
}

#[tauri::command]
fn track_toggle() -> Result<TrackState, String> {
    if trial_owns_track() {
        return Err("a blind trial owns the player — finish or cancel it first".into());
    }
    if !track_toggle_inner() {
        return Err("no track playing".into());
    }
    Ok(track_state_now())
}

#[tauri::command]
fn track_seek(seconds: f64) -> Result<TrackState, String> {
    use std::sync::atomic::Ordering::Relaxed;
    if trial_owns_track() {
        return Err("a blind trial owns the player — finish or cancel it first".into());
    }
    {
        let guard = TRACK.lock().unwrap();
        let s = guard.as_ref().ok_or("no track playing")?;
        let frame = (seconds.max(0.0) * s.shared.rate as f64) as u64;
        s.shared
            .pos
            .store(frame.min(s.shared.total_frames), Relaxed);
    }
    Ok(track_state_now())
}

/// Enter/leave scrub mode (held C): the cursor is the playhead and the audio
/// thread chases it at motion velocity (reverse included). Leaving restores
/// normal transport: playing continues from the cursor; paused stays paused.
#[tauri::command]
fn track_scrub(on: bool) -> Result<(), String> {
    use std::sync::atomic::Ordering::Relaxed;
    if trial_owns_track() {
        return Err("a blind trial owns the player — finish or cancel it first".into());
    }
    let guard = TRACK.lock().unwrap();
    let s = guard.as_ref().ok_or("no track playing")?;
    s.shared.scrub.store(on, Relaxed);
    Ok(())
}

/// Scrub feel (Clip Studio room settings): tau = chase time constant in ms,
/// max = catch-up rate ceiling in multiples of real time. Process-wide and
/// live — takes effect mid-scrub; no session required.
#[tauri::command]
fn track_scrub_params(tau_ms: f64, max_speed: f64) -> Result<(), String> {
    use std::sync::atomic::Ordering::Relaxed;
    if !tau_ms.is_finite() || !max_speed.is_finite() {
        return Err("scrub params must be finite".into());
    }
    let tau_s = (tau_ms / 1000.0).clamp(engine::SCRUB_TAU_RANGE.0, engine::SCRUB_TAU_RANGE.1);
    let max = max_speed.clamp(engine::SCRUB_MAX_RANGE.0, engine::SCRUB_MAX_RANGE.1);
    engine::SCRUB_TAU_BITS.store(tau_s.to_bits(), Relaxed);
    engine::SCRUB_MAX_BITS.store(max.to_bits(), Relaxed);
    Ok(())
}

#[tauri::command]
fn track_stop() -> TrackState {
    if trial_owns_track() {
        // The trial's stream is not the user's to stop from the transport.
        return track_state_now();
    }
    track_stop_inner();
    track_state_now()
}

// ---------------- the waveform viewer (M4) ----------------

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Waveform {
    duration_s: f64,
    mins: Vec<f32>,
    maxs: Vec<f32>,
}

/// Computed peaks memo — ~16 KB per track, evicted on delete.
static WAVES: std::sync::Mutex<std::collections::BTreeMap<i64, Waveform>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

/// Min/max peaks over the decoded PCM (the UI draws, Rust computes —
/// ADR-0005). Decodes first if the cache is cold.
#[tauri::command]
async fn track_waveform(id: i64, buckets: usize) -> Result<Waveform, String> {
    if let Some(w) = WAVES.lock().unwrap().get(&id) {
        return Ok(w.clone());
    }
    tauri::async_runtime::spawn_blocking(move || -> Result<Waveform, String> {
        let rate = 48000u32;
        let pcm = prepare_pcm(id, rate, || {})?;
        let frames = pcm.len() / 2;
        if frames == 0 {
            return Err("this file decoded to nothing — is it audio?".into());
        }
        // buckets == 0 → auto: ~128 samples (2.7 ms) per bucket, so the
        // timeline stays sharp deep into a zoom without re-fetching.
        let buckets = if buckets == 0 {
            (frames / 128).clamp(2000, 240_000)
        } else {
            buckets.clamp(100, 240_000)
        };
        let mut mins = vec![0f32; buckets];
        let mut maxs = vec![0f32; buckets];
        for (b, (mn_out, mx_out)) in mins.iter_mut().zip(maxs.iter_mut()).enumerate() {
            let start = b * frames / buckets;
            let end = (((b + 1) * frames) / buckets).max(start + 1).min(frames);
            let (mut mn, mut mx) = (f32::MAX, f32::MIN);
            for f in start..end {
                let l = pcm[f * 2];
                let r = pcm[f * 2 + 1];
                mn = mn.min(l.min(r));
                mx = mx.max(l.max(r));
            }
            *mn_out = mn.max(-1.0);
            *mx_out = mx.min(1.0);
        }
        let w = Waveform {
            duration_s: frames as f64 / rate as f64,
            mins,
            maxs,
        };
        WAVES.lock().unwrap().insert(id, w.clone());
        Ok(w)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SampleWindow {
    rate: u32,
    start_s: f64,
    mono: Vec<f32>,
}

/// One-track PCM memo so deep-zoom sample windows don't reload 100 MB
/// per pan. Keyed (track, rate); replaced when another track is inspected.
type PcmMemoEntry = (i64, u32, std::sync::Arc<Vec<f32>>);
static PCM_MEMO: std::sync::Mutex<Option<PcmMemoEntry>> = std::sync::Mutex::new(None);

/// The analysis-side PCM for a track (48 kHz canon): memoized, decoded on
/// demand. Shared by sample windows, the spectrogram, and the FFT pane.
fn pcm_for(id: i64) -> Result<std::sync::Arc<Vec<f32>>, String> {
    let rate = 48000u32;
    {
        let guard = PCM_MEMO.lock().unwrap();
        if let Some(p) = guard
            .as_ref()
            .filter(|(mid, mrate, _)| *mid == id && *mrate == rate)
            .map(|(_, _, p)| p.clone())
        {
            return Ok(p);
        }
    }
    let p = prepare_pcm(id, rate, || {})?;
    *PCM_MEMO.lock().unwrap() = Some((id, rate, p.clone()));
    Ok(p)
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SpectrogramDto {
    cols: usize,
    rows: usize,
    duration_s: f64,
    min_db: f64,
    max_db: f64,
    /// Frequency axis: true = linear 0–20 kHz, false = log 20 Hz–20 kHz.
    /// Self-describing so every renderer labels and reads out correctly.
    linear: bool,
    /// Row-major u8 grid, base64 — row 0 = lowest frequency.
    data: String,
}

type SpecKey = (i64, usize, i64, bool);
static SPECS: std::sync::Mutex<std::collections::BTreeMap<SpecKey, SpectrogramDto>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

/// The whole-track spectrogram, memoized per (track, window, floor, axis).
/// The window, floor, and axis are Clip Studio room settings.
#[tauri::command]
async fn track_spectrogram(
    id: i64,
    win: Option<usize>,
    floor_db: Option<f64>,
    linear: Option<bool>,
) -> Result<SpectrogramDto, String> {
    let win = win.unwrap_or(2048).clamp(256, 16384);
    let floor = floor_db.unwrap_or(analysis::SPEC_DB_MIN);
    let linear = linear.unwrap_or(false);
    let key: SpecKey = (id, win, floor as i64, linear);
    if let Some(s) = SPECS.lock().unwrap().get(&key) {
        return Ok(s.clone());
    }
    tauri::async_runtime::spawn_blocking(move || -> Result<SpectrogramDto, String> {
        use base64::Engine as _;
        let rate = 48000u32;
        let pcm = pcm_for(id)?;
        let frames = pcm.len() / 2;
        let spec = analysis::spectrogram(&pcm, rate, 2400, 160, win, floor, linear);
        let dto = SpectrogramDto {
            cols: spec.cols,
            rows: spec.rows,
            duration_s: frames as f64 / rate as f64,
            min_db: floor,
            max_db: analysis::SPEC_DB_MAX,
            linear,
            data: base64::engine::general_purpose::STANDARD.encode(&spec.data),
        };
        SPECS.lock().unwrap().insert(key, dto.clone());
        Ok(dto)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Spectrum at the playhead for the FFT pane (log-spaced dB points).
#[tauri::command]
async fn track_fft(id: i64, t_s: f64, points: usize) -> Result<Vec<f64>, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<f64>, String> {
        let pcm = pcm_for(id)?;
        Ok(analysis::spectrum_at(
            &pcm,
            48000,
            t_s,
            points.clamp(32, 512),
        ))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Current transport state — how satellite scope windows orient on open.
#[tauri::command]
fn track_status() -> TrackState {
    track_state_now()
}

/// Raw samples for a visible window — the timeline switches to these past
/// the peak-bucket resolution, down to single samples (mono mix of L/R).
#[tauri::command]
async fn track_samples(id: i64, start_s: f64, span_s: f64) -> Result<SampleWindow, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<SampleWindow, String> {
        let rate = 48000u32;
        let pcm = pcm_for(id)?;
        let frames = pcm.len() / 2;
        let span = span_s.clamp(0.01, 8.0);
        let f0 = ((start_s.max(0.0) * rate as f64) as usize).min(frames);
        let f1 = (f0 + (span * rate as f64).ceil() as usize).min(frames);
        let mono: Vec<f32> = (f0..f1)
            .map(|f| (pcm[f * 2] + pcm[f * 2 + 1]) * 0.5)
            .collect();
        Ok(SampleWindow {
            rate,
            start_s: f0 as f64 / rate as f64,
            mono,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

// ---------------- Settings (v1: the approved artboard) ----------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsState {
    reference_db: f64,
    level_matching: bool,
    apo_install_path: Option<String>,
    apo_config_path: Option<String>,
}

#[tauri::command]
fn settings_state() -> SettingsState {
    let apo = apo::detect().ok();
    SettingsState {
        reference_db: reference_db(),
        level_matching: level_matching(),
        apo_install_path: apo.as_ref().map(|i| i.install_path.display().to_string()),
        apo_config_path: apo.as_ref().map(|i| i.config_path.display().to_string()),
    }
}

/// Set the global reference loudness (Q-06/Q-16 placeholder — the noise
/// calibration flow replaces the raw number once the signal generator exists).
/// Re-applies the current side so the change is immediately audible.
#[tauri::command]
fn set_reference_db(db: f64) -> Result<EqState, String> {
    if TRIAL.lock().unwrap().is_some() {
        return Err("a blind test is running — finish or cancel it first".into());
    }
    write_state_field("referenceDb", serde_json::json!(db.clamp(-30.0, 0.0)));
    apply_side(&ab_side())?;
    eq_state()
}

/// The honesty switch. Refused mid-ABX: changing levels under a running blind
/// test would corrupt what the trials are measuring.
#[tauri::command]
fn set_level_matching(on: bool) -> Result<EqState, String> {
    if TRIAL.lock().unwrap().is_some() {
        return Err("a blind test is running — finish or cancel it first".into());
    }
    write_state_field("levelMatching", serde_json::json!(on));
    apply_side(&ab_side())?;
    eq_state()
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
    /// TB-08: dB by which the active chain sits ABOVE the reference because
    /// the clip-safe cap won. 0 = fully matched.
    shortfall_db: f64,
}

#[tauri::command]
fn ab_info() -> AbInfo {
    let (_, shortfall_db) = matched_preamp_full(&specs_of(&active_chain()));
    AbInfo {
        side: ab_side(),
        match_db: matched_preamp(&[]),
        shortfall_db,
    }
}

#[tauri::command]
fn ab_set(side: String) -> Result<EqState, String> {
    let side = if side == "b" { "b" } else { "a" };
    if track_engine_side_set(side) {
        set_ab_side(side);
        return eq_state();
    }
    apply_side(side)?;
    set_ab_side(side);
    eq_state()
}

/// Flip A/B (hotkey + tray path); emits `ab-changed` with the new side.
/// During an ABX session the hotkey cycles the audition target instead.
fn ab_flip(app: &tauri::AppHandle) {
    use tauri::Emitter;
    {
        let mut guard = TRIAL.lock().unwrap();
        if let Some(session) = guard.as_mut() {
            let next = match (session.protocol, session.audition.as_str()) {
                (Protocol::Abx, "a") => "b",
                (Protocol::Abx, "b") => "x",
                (Protocol::Abx, _) => "a",
                (Protocol::Pref, "1") => "2",
                (Protocol::Pref, _) => "1",
            };
            if trial_apply_audition(session, next).is_ok() {
                let _ = app.emit("abx-audition", next);
            }
            return;
        }
    }
    let next = if ab_side() == "a" { "b" } else { "a" };
    // Third branch: an exclusive track session flips inside the engine.
    if track_engine_side_set(next) {
        set_ab_side(next);
        let _ = app.emit("ab-changed", next);
        return;
    }
    if apply_side(next).is_ok() {
        set_ab_side(next);
        let _ = app.emit("ab-changed", next);
    }
}

// ---------------- the trial room engine (ABX + preference) ----------------
//
// The session lives server-side only; X/slot assignments never reach the UI
// while the session runs (that's the blinding). Everything is journaled and
// the finished session persists with full labels for replay (Q-05).
//
// One engine, three axes: protocol (ABX discrimination | blind preference),
// material (system audio via config swap | clips through the track engine's
// testing mode — ADR-0011's third play method), and stop rule (fixed N |
// Wald SPRT sequential). Lock order where both are held: TRIAL → TRACK.

#[derive(Clone, Copy, PartialEq)]
enum Protocol {
    Abx,
    Pref,
}

/// One planned trial's material + leveling, measured at session start.
/// Names are denormalized: provenance survives clip/track deletion.
#[derive(Clone)]
struct TrialClip {
    clip_id: i64,
    track_id: i64,
    clip_name: String,
    track_title: String,
    frame_in: u64,
    frame_out: u64,
    /// True-LUFS trim bringing bus B to bus A's loudness on this clip.
    trim_b_db: f64,
    /// >0 when the TB-06 headroom cap kept the trim short of a perfect match.
    trim_shortfall_db: f64,
    lufs_a: f64,
    lufs_b: f64,
    /// Whole-stream gain while this clip's track plays (per-track, attenuate-only).
    gain_db: f64,
}

enum Material {
    /// Config-swap auditions through APO — the classic system-wide test.
    System,
    /// The trial owns the track engine; each trial solos one clip on loop
    /// with both contender chains running in-engine (TB-12 dual bus).
    Clips {
        battery_id: Option<i64>,
        battery_name: Option<String>,
        plan: Vec<TrialClip>,
        rate: u32,
        /// Device lost mid-session: auditions/votes refused until resume.
        suspended: bool,
    },
}

enum StopRule {
    FixedN {
        planned: usize,
    },
    /// Declared before the first vote and recorded — the stop rule is part
    /// of the test's provenance (no optional-stopping p-hacking).
    Sprt {
        p1: f64,
        alpha: f64,
        beta: f64,
        max_trials: usize,
    },
}

const SPRT_P1: f64 = 0.75;
const SPRT_ALPHA: f64 = 0.05;
const SPRT_BETA: f64 = 0.05;
const SPRT_CAP: usize = 40;

struct TrialSession {
    id: String,
    protocol: Protocol,
    material: Material,
    stop_rule: StopRule,
    a_name: String,
    b_name: String,
    /// The two competing chains, captured at start. Every audition plays
    /// level-matched (config path: shared matched-preamp write; clips path:
    /// matched preamps + measured trim), so A and B land at the reference
    /// loudness by construction.
    a: Vec<ChainFilter>,
    b: Vec<ChainFilter>,
    /// ABX: trial t's X is A.  Pref: trial t's slot "1" is A.
    assignments: Vec<bool>,
    /// ABX: user said "X is A".  Pref: user picked slot "1".
    answers: Vec<bool>,
    stats_viewed: Vec<usize>,
    audition: String, // ABX: "a"|"b"|"x"   Pref: "1"|"2"
    /// Whether the honesty switch was on when the session started (provenance).
    level_matched: bool,
    started_ms: u64,
}

static TRIAL: std::sync::Mutex<Option<TrialSession>> = std::sync::Mutex::new(None);

/// Raised while a clips-material trial owns the track engine: public track
/// commands and live chain pushes are refused/skipped, and the trial drives
/// TRACK through the same internals.
static TRIAL_OWNS_TRACK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn trial_owns_track() -> bool {
    TRIAL_OWNS_TRACK.load(std::sync::atomic::Ordering::Relaxed)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Which chain the listener should hear for an audition target.
fn trial_hear_a(session: &TrialSession, target: &str) -> bool {
    let trial = session.answers.len();
    let assigned = *session.assignments.get(trial).unwrap_or(&true);
    match (session.protocol, target) {
        (Protocol::Abx, "a") => true,
        (Protocol::Abx, "b") => false,
        (Protocol::Abx, _) => assigned, // "x"
        // Pref: slot "1" is A exactly when assigned; "2" is the mirror.
        (Protocol::Pref, t) => (t == "1") == assigned,
    }
}

/// Make what the listener should hear right now actually play.
fn trial_apply_audition(session: &mut TrialSession, target: &str) -> Result<(), String> {
    let hear_a = trial_hear_a(session, target);
    match &session.material {
        Material::System => {
            activate_chain(if hear_a { &session.a } else { &session.b })?;
        }
        Material::Clips { suspended, .. } => {
            if *suspended {
                return Err("the audio device was lost — resume or finish the session".into());
            }
            let guard = TRACK.lock().unwrap();
            let s = guard
                .as_ref()
                .ok_or("trial playback is not running — resume the session")?;
            // The engine's 15 ms equal-power crossfade does the switch —
            // sample-accurate, APO out of the path (TB-12).
            s.shared
                .side_b
                .store(!hear_a, std::sync::atomic::Ordering::Relaxed);
        }
    }
    session.audition = target.to_string();
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TrialState {
    active: bool,
    protocol: &'static str, // "abx" | "pref"
    material: &'static str, // "system" | "clips"
    a_name: String,
    b_name: String,
    planned: usize,
    adaptive: bool,
    answered: usize,
    audition: String,
    level_matched: bool,
    /// Correct count so far — present only after an explicit reveal (TB-24).
    /// For preference: the preferred-A count.
    running_correct: Option<usize>,
    // Clips material, current trial (None for system material):
    #[serde(skip_serializing_if = "Option::is_none")]
    clip_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    track_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    clip_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    clip_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    battery_name: Option<String>,
    suspended: bool,
}

fn trial_state_of(session: &TrialSession, revealed: bool) -> TrialState {
    let trial = session.answers.len();
    let (clip_name, track_title, clip_index, clip_count, battery_name, suspended) = match &session
        .material
    {
        Material::System => (None, None, None, None, None, false),
        Material::Clips {
            plan,
            battery_name,
            suspended,
            ..
        } => {
            let cur = plan.get(trial.min(plan.len().saturating_sub(1)));
            let distinct: std::collections::HashSet<i64> = plan.iter().map(|c| c.clip_id).collect();
            (
                cur.map(|c| c.clip_name.clone()),
                cur.map(|c| c.track_title.clone()),
                cur.map(|c| {
                    // Position within the distinct battery, 1-based.
                    let mut seen = std::collections::HashSet::new();
                    let mut idx = 0;
                    for p in plan.iter().take(trial + 1) {
                        if seen.insert(p.clip_id) {
                            idx += 1;
                        }
                        if p.clip_id == c.clip_id {
                            break;
                        }
                    }
                    idx.max(1)
                }),
                Some(distinct.len()),
                battery_name.clone(),
                *suspended,
            )
        }
    };
    TrialState {
        active: true,
        protocol: match session.protocol {
            Protocol::Abx => "abx",
            Protocol::Pref => "pref",
        },
        material: match session.material {
            Material::System => "system",
            Material::Clips { .. } => "clips",
        },
        a_name: session.a_name.clone(),
        b_name: session.b_name.clone(),
        level_matched: session.level_matched,
        planned: match session.stop_rule {
            StopRule::FixedN { planned } => planned,
            StopRule::Sprt { max_trials, .. } => max_trials,
        },
        adaptive: matches!(session.stop_rule, StopRule::Sprt { .. }),
        answered: session.answers.len(),
        audition: session.audition.clone(),
        running_correct: if revealed {
            Some(abx_correct(session))
        } else {
            None
        },
        clip_name,
        track_title,
        clip_index,
        clip_count,
        battery_name,
        suspended,
    }
}

/// ABX: correct answers. Pref: preferred-A count (picked the slot holding A).
fn abx_correct(session: &TrialSession) -> usize {
    session
        .answers
        .iter()
        .zip(&session.assignments)
        .filter(|(ans, actual)| ans == actual)
        .count()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrialSpec {
    protocol: Option<String>, // "abx" (default) | "pref"
    material: Option<MaterialSpec>,
    a: Option<Vec<FilterIn>>,
    b: Option<Vec<FilterIn>>,
    a_name: Option<String>,
    b_name: Option<String>,
    stop: Option<StopSpec>,
}

#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum MaterialSpec {
    System,
    Clips {
        battery_id: Option<i64>,
        clip_ids: Option<Vec<i64>>,
    },
}

#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum StopSpec {
    FixedN { trials: usize },
    Sprt { max_trials: Option<usize> },
}

/// Shuffle in place with the session RNG (Fisher–Yates).
fn shuffle<T>(v: &mut [T], rng: &mut fletcher_core::stats::Xorshift) {
    for i in (1..v.len()).rev() {
        let j = (rng.next_u64() % (i as u64 + 1)) as usize;
        v.swap(i, j);
    }
}

/// Resolve chains + names for a trial (both explicit, or active-vs-flat).
fn trial_chains(
    a: Option<Vec<FilterIn>>,
    b: Option<Vec<FilterIn>>,
    a_name: Option<String>,
    b_name: Option<String>,
) -> Result<(Vec<ChainFilter>, Vec<ChainFilter>, String, String), String> {
    match (a, b) {
        (Some(a), Some(b)) => Ok((
            chain_of(&a)?,
            chain_of(&b)?,
            a_name.unwrap_or_else(|| "A".into()),
            b_name.unwrap_or_else(|| "B".into()),
        )),
        (None, None) => {
            let chain = active_chain();
            if chain.iter().filter(|f| f.enabled).count() == 0 {
                return Err("activate a preset with at least one enabled filter first — A and B would sound identical".into());
            }
            Ok((
                chain,
                Vec::new(),
                active_preset().unwrap_or_else(|| "Fletcher chain".into()),
                "Flat".into(),
            ))
        }
        _ => Err("provide both chains or neither".into()),
    }
}

/// The contender chains + honesty snapshot a spawn needs.
struct TrialChains<'a> {
    a: &'a [ChainFilter],
    b: &'a [ChainFilter],
    level_matched: bool,
}

/// Spawn the track engine for one trial clip: exclusive, both contender
/// chains in-engine, clip soloed on loop, opening side set blind. On stream
/// error the trial suspends instead of dying (answers survive — ADR-0006).
fn spawn_trial_clip(
    app: &tauri::AppHandle,
    session_id: &str,
    chains: TrialChains<'_>,
    clip: &TrialClip,
    rate: u32,
    hear_a: bool,
) -> Result<(), String> {
    use std::sync::atomic::Ordering::Relaxed;
    let pcm = prepare_pcm(clip.track_id, rate, || {})?;
    let total = (pcm.len() / 2) as u64;
    let start = clip.frame_in.min(total.saturating_sub(1));
    let session = engine::TrackSession::new(
        clip.track_id,
        fletcher_core::playback::OutputMode::Exclusive,
        rate,
        total,
        start,
    );
    session.shared.loop_a.store(clip.frame_in, Relaxed);
    session
        .shared
        .loop_b
        .store(clip.frame_out.min(total), Relaxed);
    session.shared.loop_on.store(true, Relaxed);
    session.shared.side_b.store(!hear_a, Relaxed);
    let (sa, sb) = (specs_of(chains.a), specs_of(chains.b));
    let (pa, pb) = (matched_preamp(&sa), matched_preamp(&sb));
    {
        let mut bus = session.shared.bus.lock().unwrap();
        *bus = engine::BusConfig {
            specs: sa,
            preamp_a_db: pa,
            specs_b: sb,
            preamp_b_db: pb,
            trim_b_db: clip.trim_b_db,
            matched: chains.level_matched,
            in_engine_eq: true,
        };
    }
    session.shared.chain_gen.fetch_add(1, Relaxed);
    let _ = TRACK_SESS.fetch_add(1, Relaxed);
    let stop_end = session.stop.clone();
    let (app_end, sid) = (app.clone(), session_id.to_string());
    session.spawn_audio(
        pcm,
        10f64.powf(clip.gain_db / 20.0),
        |_info| {},
        move |err| {
            use tauri::Emitter;
            // Sequential locks, never nested: TRACK first, then TRIAL.
            {
                let mut guard = TRACK.lock().unwrap();
                if let Some(cur) = guard.as_ref() {
                    if std::sync::Arc::ptr_eq(&cur.stop, &stop_end) {
                        *guard = None;
                    }
                }
            }
            if let Some(e) = err {
                let mut guard = TRIAL.lock().unwrap();
                if let Some(s) = guard.as_mut() {
                    if s.id == sid {
                        if let Material::Clips { suspended, .. } = &mut s.material {
                            *suspended = true;
                        }
                        let _ = app_end.emit("trial-error", e);
                    }
                }
            }
        },
    );
    *TRACK.lock().unwrap() = Some(session);
    Ok(())
}

/// Start a session from a full spec. Clip material decodes + measures before
/// the first trial, so it runs blocking.
#[tauri::command]
async fn trial_start(app: tauri::AppHandle, spec: TrialSpec) -> Result<TrialState, String> {
    tauri::async_runtime::spawn_blocking(move || trial_start_inner(&app, spec))
        .await
        .map_err(|e| e.to_string())?
}

fn trial_start_inner(app: &tauri::AppHandle, spec: TrialSpec) -> Result<TrialState, String> {
    let protocol = match spec.protocol.as_deref() {
        None | Some("abx") => Protocol::Abx,
        Some("pref") => Protocol::Pref,
        Some(other) => return Err(format!("unknown protocol: {other}")),
    };
    let stop_rule = match spec.stop {
        None => StopRule::FixedN { planned: 16 },
        Some(StopSpec::FixedN { trials }) => StopRule::FixedN {
            planned: trials.clamp(4, 100),
        },
        Some(StopSpec::Sprt { max_trials }) => {
            if protocol == Protocol::Pref {
                return Err(
                    "preference runs a fixed count — sequential stopping would inflate the preference score"
                        .into(),
                );
            }
            StopRule::Sprt {
                p1: SPRT_P1,
                alpha: SPRT_ALPHA,
                beta: SPRT_BETA,
                max_trials: max_trials.unwrap_or(SPRT_CAP).clamp(8, 100),
            }
        }
    };
    let (chain_a, chain_b, name_a, name_b) =
        trial_chains(spec.a, spec.b, spec.a_name, spec.b_name)?;
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
    if TRIAL.lock().unwrap().is_some() {
        return Err("a blind test is already running — finish or cancel it first".into());
    }
    // Leveling noise and blind trials must never overlap (TB-26 class).
    stop_cal_noise();

    let cap = match stop_rule {
        StopRule::FixedN { planned } => planned,
        StopRule::Sprt { max_trials, .. } => max_trials,
    };
    let mut rng = fletcher_core::stats::Xorshift::new(now_ms() | 1);
    let assignments: Vec<bool> = (0..cap).map(|_| rng.next_bool()).collect();
    let opening = match protocol {
        Protocol::Abx => "a",
        Protocol::Pref => "1",
    };
    let id_prefix = match protocol {
        Protocol::Abx => "abx",
        Protocol::Pref => "pref",
    };

    let material = match spec.material {
        None | Some(MaterialSpec::System) => {
            if TRACK.lock().unwrap().is_some() {
                return Err(
                    "stop track playback first — system-wide tests and the track engine can't share the device"
                        .into(),
                );
            }
            Material::System
        }
        Some(MaterialSpec::Clips {
            battery_id,
            clip_ids,
        }) => trial_build_clips_material(&chain_a, &chain_b, battery_id, clip_ids, cap, &mut rng)?,
    };

    let mut session = TrialSession {
        id: format!("{id_prefix}-{}", now_ms()),
        protocol,
        material,
        stop_rule,
        a_name: name_a,
        b_name: name_b,
        a: chain_a,
        b: chain_b,
        assignments,
        answers: Vec::new(),
        stats_viewed: Vec::new(),
        audition: opening.into(),
        level_matched: level_matching(),
        started_ms: now_ms(),
    };

    match &session.material {
        Material::System => {
            trial_apply_audition(&mut session, opening)?;
        }
        Material::Clips { plan, rate, .. } => {
            // Takeover: the trial owns the track engine from here on.
            let prior_done = track_stop_inner();
            TRIAL_OWNS_TRACK.store(true, std::sync::atomic::Ordering::Relaxed);
            if let Some(done) = prior_done {
                let t0 = std::time::Instant::now();
                while !done.load(std::sync::atomic::Ordering::Relaxed)
                    && t0.elapsed() < std::time::Duration::from_secs(3)
                {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            }
            let hear_a = trial_hear_a(&session, opening);
            let clip = plan[0].clone();
            let rate = *rate;
            if let Err(e) = spawn_trial_clip(
                app,
                &session.id,
                TrialChains {
                    a: &session.a,
                    b: &session.b,
                    level_matched: session.level_matched,
                },
                &clip,
                rate,
                hear_a,
            ) {
                TRIAL_OWNS_TRACK.store(false, std::sync::atomic::Ordering::Relaxed);
                return Err(e);
            }
        }
    }

    let state = trial_state_of(&session, false);
    *TRIAL.lock().unwrap() = Some(session);
    Ok(state)
}

/// Resolve, order, and level-measure a battery (or ad-hoc clip list) into a
/// full trial plan. Shuffled but grouped by track: the blind protects the
/// assignment (iid per trial regardless of material order), and grouping
/// minimizes exclusive open/close churn — the flakiest driver operation.
fn trial_build_clips_material(
    chain_a: &[ChainFilter],
    chain_b: &[ChainFilter],
    battery_id: Option<i64>,
    clip_ids: Option<Vec<i64>>,
    cap: usize,
    rng: &mut fletcher_core::stats::Xorshift,
) -> Result<Material, String> {
    let conn = library_db()?;
    let (ids, battery_name) = match (battery_id, clip_ids) {
        (Some(bid), _) => {
            let name: String = conn
                .query_row(
                    "SELECT name FROM batteries WHERE id = ?1",
                    rusqlite::params![bid],
                    |r| r.get(0),
                )
                .map_err(|_| "battery not found".to_string())?;
            let mut stmt = conn
                .prepare("SELECT clip_id FROM battery_clips WHERE battery_id = ?1 ORDER BY ord")
                .map_err(|e| e.to_string())?;
            let ids: Vec<i64> = stmt
                .query_map(rusqlite::params![bid], |r| r.get(0))
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            (ids, Some(name))
        }
        (None, Some(ids)) => (ids, None),
        (None, None) => return Err("pick a battery or clips to test on".into()),
    };
    if ids.is_empty() {
        return Err("that battery is empty — add clips to it first".into());
    }
    // Fetch clip + track rows (names denormalized for provenance).
    struct ClipRef {
        clip_id: i64,
        track_id: i64,
        name: String,
        title: String,
        t_in: f64,
        t_out: f64,
    }
    let mut clips: Vec<ClipRef> = Vec::new();
    for cid in &ids {
        let row = conn
            .query_row(
                "SELECT c.id, c.track_id, c.name, t.title, c.t_in, c.t_out
                 FROM clips c JOIN tracks t ON t.id = c.track_id WHERE c.id = ?1",
                rusqlite::params![cid],
                |r| {
                    Ok(ClipRef {
                        clip_id: r.get(0)?,
                        track_id: r.get(1)?,
                        name: r.get(2)?,
                        title: r.get(3)?,
                        t_in: r.get(4)?,
                        t_out: r.get(5)?,
                    })
                },
            )
            .map_err(|_| format!("clip #{cid} no longer exists"))?;
        if row.t_out - row.t_in < 0.5 {
            return Err(format!(
                "clip \"{}\" is under 0.5 s — too short for a blind switch",
                row.name
            ));
        }
        clips.push(row);
    }
    drop(conn);

    // Exclusive only: the shared path would run the engine's output through
    // APO — double-processing (TB-03). No degraded mode.
    let rate = fletcher_core::playback::probe_rate(fletcher_core::playback::OutputMode::Exclusive)
        .map_err(|_| {
            "blind clip trials need exclusive mode (the shared path would double-process through APO) — the device refused it"
                .to_string()
        })?;

    // Measure each distinct clip once: LUFS through both chains over the clip
    // region, trim B to match A (TB-06: positive trims capped at post-chain
    // peak headroom − 0.3 dB, shortfall recorded). Per-track stream gain from
    // the loudest clip so nothing exceeds the reference target and clips keep
    // their natural relative loudness.
    let (sa, sb) = (specs_of(chain_a), specs_of(chain_b));
    let (pa, pb) = (matched_preamp(&sa), matched_preamp(&sb));
    let target = -16.0 + (reference_db() + 8.0);
    let mut measured: std::collections::HashMap<i64, TrialClip> = std::collections::HashMap::new();
    let mut track_gain: std::collections::HashMap<i64, f64> = std::collections::HashMap::new();
    let mut track_ids: Vec<i64> = Vec::new();
    for c in &clips {
        if !track_ids.contains(&c.track_id) {
            track_ids.push(c.track_id);
        }
    }
    for tid in &track_ids {
        let pcm = prepare_pcm(*tid, rate, || {})?;
        let total = (pcm.len() / 2) as u64;
        for c in clips.iter().filter(|c| c.track_id == *tid) {
            let fi = ((c.t_in * rate as f64) as u64).min(total.saturating_sub(1));
            let fo = ((c.t_out * rate as f64) as u64).clamp(fi + 1, total);
            let mut slice: Vec<f32> = pcm[(fi as usize) * 2..(fo as usize) * 2].to_vec();
            // R128 gating needs runway; tiling matches what looping sounds like.
            let min_len = (rate as usize) * 3 * 2;
            while slice.len() < min_len {
                let take = (min_len - slice.len()).min((fo - fi) as usize * 2);
                let head: Vec<f32> = slice[..take].to_vec();
                slice.extend_from_slice(&head);
            }
            let (lufs_a, _) = engine::lufs_peak_through(&slice, rate, &sa, pa)?;
            let (lufs_b, peak_b) = engine::lufs_peak_through(&slice, rate, &sb, pb)?;
            let want = lufs_a - lufs_b;
            let (trim, shortfall) = if want > 0.0 {
                let headroom = (-peak_b - 0.3).max(0.0);
                (want.min(headroom), (want - headroom).max(0.0))
            } else {
                (want, 0.0)
            };
            let g = (target - lufs_a).min(0.0);
            let e = track_gain.entry(*tid).or_insert(g);
            *e = e.min(g);
            measured.insert(
                c.clip_id,
                TrialClip {
                    clip_id: c.clip_id,
                    track_id: c.track_id,
                    clip_name: c.name.clone(),
                    track_title: c.title.clone(),
                    frame_in: fi,
                    frame_out: fo,
                    trim_b_db: trim,
                    trim_shortfall_db: shortfall,
                    lufs_a,
                    lufs_b,
                    gain_db: 0.0, // filled below from the track minimum
                },
            );
        }
        // PCM Arc drops here — one track resident at a time.
    }
    for tc in measured.values_mut() {
        tc.gain_db = *track_gain.get(&tc.track_id).unwrap_or(&0.0);
    }

    // Order: shuffle tracks, shuffle clips within each track, repeat fresh
    // passes until the plan covers the trial cap, truncate.
    let mut plan: Vec<TrialClip> = Vec::new();
    while plan.len() < cap {
        let mut order = track_ids.clone();
        shuffle(&mut order, rng);
        for tid in order {
            let mut group: Vec<i64> = clips
                .iter()
                .filter(|c| c.track_id == tid)
                .map(|c| c.clip_id)
                .collect();
            shuffle(&mut group, rng);
            for cid in group {
                plan.push(measured[&cid].clone());
            }
        }
    }
    plan.truncate(cap);

    Ok(Material::Clips {
        battery_id,
        battery_name,
        plan,
        rate,
        suspended: false,
    })
}

/// Classic entry, kept for existing callers: system material, fixed N.
#[tauri::command]
fn abx_start(
    app: tauri::AppHandle,
    trials: usize,
    a: Option<Vec<FilterIn>>,
    b: Option<Vec<FilterIn>>,
    a_name: Option<String>,
    b_name: Option<String>,
) -> Result<TrialState, String> {
    trial_start_inner(
        &app,
        TrialSpec {
            protocol: Some("abx".into()),
            material: Some(MaterialSpec::System),
            a,
            b,
            a_name,
            b_name,
            stop: Some(StopSpec::FixedN { trials }),
        },
    )
}

#[tauri::command]
fn abx_audition(target: String) -> Result<TrialState, String> {
    let mut guard = TRIAL.lock().unwrap();
    let session = guard.as_mut().ok_or("no session running")?;
    let valid = match session.protocol {
        Protocol::Abx => matches!(target.as_str(), "a" | "b" | "x"),
        // Open A/B auditioning would unblind the slot — 1/2 only.
        Protocol::Pref => matches!(target.as_str(), "1" | "2"),
    };
    if !valid {
        return Err("unknown target".into());
    }
    trial_apply_audition(session, &target)?;
    Ok(trial_state_of(session, false))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TrialLogEntry {
    /// ABX: X was A on this trial. Pref: slot "1" held A.
    x_was_a: bool,
    /// ABX: the listener said "X is A". Pref: the listener picked slot "1".
    answered_a: bool,
    /// ABX: right answer. Pref: this trial preferred A.
    correct: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    clip_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    clip_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    track_id: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TrialResult {
    id: String,
    protocol: &'static str,
    a_name: String,
    b_name: String,
    /// Full provenance: what A and B actually were, and the reference both
    /// were matched to — a session replays meaningfully years later.
    a_chain: Vec<PastedFilter>,
    b_chain: Vec<PastedFilter>,
    reference_db: f64,
    level_matched: bool,
    trials: usize,
    /// ABX only: correct count + one-sided exact binomial p.
    #[serde(skip_serializing_if = "Option::is_none")]
    correct: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    p_value: Option<f64>,
    /// Preference only: no correct answer exists (TB-15) — the preferred-A
    /// count and a TWO-sided exact test against no-preference.
    #[serde(skip_serializing_if = "Option::is_none")]
    preferred_a: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    p_value_two_sided: Option<f64>,
    material: serde_json::Value,
    stop_rule: serde_json::Value,
    /// Sequential (SPRT) sessions: p was computed at a data-dependent stop —
    /// nominal, never the primary criterion. The decision carries the
    /// calibrated error rates.
    sequential: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    decision: Option<&'static str>, // "difference" | "noDifference" | "cap"
    finished_early: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    playback: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    trims: Vec<serde_json::Value>,
    stats_viewed: Vec<usize>,
    log: Vec<TrialLogEntry>,
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

fn trial_result_of(
    session: &TrialSession,
    decision: Option<&'static str>,
    finished_early: bool,
) -> TrialResult {
    let k = abx_correct(session);
    let n = session.answers.len();
    let (material, playback, trims) = match &session.material {
        Material::System => (serde_json::json!({ "kind": "system" }), None, Vec::new()),
        Material::Clips {
            battery_id,
            battery_name,
            plan,
            rate,
            ..
        } => {
            let mut seen = std::collections::HashSet::new();
            let mut clip_ids = Vec::new();
            let mut trims = Vec::new();
            for c in plan {
                if seen.insert(c.clip_id) {
                    clip_ids.push(c.clip_id);
                    trims.push(serde_json::json!({
                        "clipId": c.clip_id,
                        "trimBDb": c.trim_b_db,
                        "trimShortfallDb": c.trim_shortfall_db,
                        "lufsA": c.lufs_a,
                        "lufsB": c.lufs_b,
                        "gainDb": c.gain_db,
                    }));
                }
            }
            (
                serde_json::json!({
                    "kind": "clips",
                    "batteryId": battery_id,
                    "batteryName": battery_name,
                    "clipIds": clip_ids,
                }),
                Some(serde_json::json!({ "mode": "exclusive", "rate": rate })),
                trims,
            )
        }
    };
    let stop_rule = match session.stop_rule {
        StopRule::FixedN { planned } => {
            serde_json::json!({ "kind": "fixedN", "planned": planned })
        }
        StopRule::Sprt {
            p1,
            alpha,
            beta,
            max_trials,
        } => serde_json::json!({
            "kind": "sprt", "p1": p1, "alpha": alpha, "beta": beta, "maxTrials": max_trials
        }),
    };
    let log = session
        .answers
        .iter()
        .zip(&session.assignments)
        .enumerate()
        .map(|(i, (ans, actual))| {
            let clip = match &session.material {
                Material::System => None,
                Material::Clips { plan, .. } => plan.get(i),
            };
            TrialLogEntry {
                x_was_a: *actual,
                answered_a: *ans,
                correct: ans == actual,
                clip_id: clip.map(|c| c.clip_id),
                clip_name: clip.map(|c| c.clip_name.clone()),
                track_id: clip.map(|c| c.track_id),
            }
        })
        .collect();
    let is_abx = session.protocol == Protocol::Abx;
    TrialResult {
        id: session.id.clone(),
        protocol: if is_abx { "abx" } else { "pref" },
        a_name: session.a_name.clone(),
        b_name: session.b_name.clone(),
        a_chain: dto_chain(&session.a),
        b_chain: dto_chain(&session.b),
        reference_db: reference_db(),
        level_matched: session.level_matched,
        trials: n,
        correct: is_abx.then_some(k),
        p_value: is_abx.then(|| fletcher_core::stats::binomial_p_one_sided(k as u32, n as u32)),
        preferred_a: (!is_abx).then_some(k),
        p_value_two_sided: (!is_abx)
            .then(|| fletcher_core::stats::binomial_p_two_sided(k as u32, n as u32)),
        material,
        stop_rule,
        sequential: matches!(session.stop_rule, StopRule::Sprt { .. }),
        decision,
        finished_early,
        playback,
        trims,
        stats_viewed: session.stats_viewed.clone(),
        log,
        started_ms: session.started_ms,
    }
}

fn sessions_dir() -> PathBuf {
    data_dir().join("sessions")
}

/// Persist a finished session and restore normal playback for its material.
/// Returns the result as a JSON value.
fn trial_finalize(
    session: &TrialSession,
    decision: Option<&'static str>,
    finished_early: bool,
) -> Result<serde_json::Value, String> {
    let result = trial_result_of(session, decision, finished_early);
    let _ = std::fs::create_dir_all(sessions_dir());
    let json = serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?;
    let _ = fsx::write_atomic(&sessions_dir().join(format!("{}.json", result.id)), &json);
    match session.material {
        Material::System => {
            let _ = apply_side("a");
            set_ab_side("a");
        }
        Material::Clips { .. } => {
            // APO was never touched — releasing the exclusive stream is the
            // whole restoration; system audio comes back by itself.
            TRIAL_OWNS_TRACK.store(false, std::sync::atomic::Ordering::Relaxed);
            track_stop_inner();
        }
    }
    serde_json::from_str(&json).map_err(|e| e.to_string())
}

/// Vote for the current trial. Returns the final result when the stop rule
/// says the session is over (fixed count reached, or SPRT decided).
#[tauri::command]
async fn abx_vote(app: tauri::AppHandle, x_is_a: bool) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || trial_vote_inner(&app, x_is_a))
        .await
        .map_err(|e| e.to_string())?
}

fn trial_vote_inner(app: &tauri::AppHandle, pick: bool) -> Result<serde_json::Value, String> {
    let mut guard = TRIAL.lock().unwrap();
    let session = guard.as_mut().ok_or("no session running")?;
    if let Material::Clips {
        suspended: true, ..
    } = session.material
    {
        return Err("the audio device was lost — resume or finish the session".into());
    }
    let cap = match session.stop_rule {
        StopRule::FixedN { planned } => planned,
        StopRule::Sprt { max_trials, .. } => max_trials,
    };
    if session.answers.len() >= cap {
        return Err("session already complete".into());
    }
    session.answers.push(pick);
    let k = abx_correct(session);
    let n = session.answers.len();
    let (done, decision) = match session.stop_rule {
        StopRule::FixedN { planned } => (n >= planned, None),
        StopRule::Sprt {
            p1,
            alpha,
            beta,
            max_trials,
        } => {
            // The server decides and finishes — bounds-vs-score never reaches
            // the UI mid-session (TB-24-clean adaptive stopping).
            use fletcher_core::stats::SprtDecision::*;
            match fletcher_core::stats::sprt_step(k as u32, n as u32, p1, alpha, beta) {
                AcceptH1 => (true, Some("difference")),
                AcceptH0 => (true, Some("noDifference")),
                Continue if n >= max_trials => (true, Some("cap")),
                Continue => (false, None),
            }
        }
    };

    if done {
        let result = trial_finalize(session, decision, false)?;
        *guard = None;
        return Ok(serde_json::json!({ "done": true, "result": result }));
    }

    // Next trial opens blind: X's fresh assignment (ABX) or slot 1 (Pref).
    let opening = match session.protocol {
        Protocol::Abx => "x",
        Protocol::Pref => "1",
    };
    match &session.material {
        Material::System => {
            trial_apply_audition(session, opening)?;
        }
        Material::Clips { plan, rate, .. } => {
            let idx = session.answers.len();
            let (next, prev_track, rate) = (plan[idx].clone(), plan[idx - 1].track_id, *rate);
            let hear_a = trial_hear_a(session, opening);
            session.audition = opening.into();
            if next.track_id == prev_track {
                // In-place retune: pause masks the seam; same stream, same gain.
                use std::sync::atomic::Ordering::Relaxed;
                let tg = TRACK.lock().unwrap();
                let s = tg
                    .as_ref()
                    .ok_or("trial playback is not running — resume the session")?;
                s.shared.paused.store(true, Relaxed);
                s.shared.loop_a.store(next.frame_in, Relaxed);
                s.shared.loop_b.store(next.frame_out, Relaxed);
                s.shared.pos.store(next.frame_in, Relaxed);
                {
                    let mut bus = s.shared.bus.lock().unwrap();
                    bus.trim_b_db = next.trim_b_db;
                }
                s.shared.chain_gen.fetch_add(1, Relaxed);
                s.shared.side_b.store(!hear_a, Relaxed);
                s.shared.paused.store(false, Relaxed);
            } else {
                // Track handoff: drop the TRIAL lock while the device turns
                // around; re-verify afterwards so a concurrent cancel wins.
                let sid = session.id.clone();
                let (a, b, lm) = (session.a.clone(), session.b.clone(), session.level_matched);
                drop(guard);
                if let Some(donef) = track_stop_inner() {
                    let t0 = std::time::Instant::now();
                    while !donef.load(std::sync::atomic::Ordering::Relaxed)
                        && t0.elapsed() < std::time::Duration::from_secs(3)
                    {
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                }
                let spawn = spawn_trial_clip(
                    app,
                    &sid,
                    TrialChains {
                        a: &a,
                        b: &b,
                        level_matched: lm,
                    },
                    &next,
                    rate,
                    hear_a,
                );
                let mut guard = TRIAL.lock().unwrap();
                match guard.as_mut() {
                    Some(s) if s.id == sid => {
                        if let Err(e) = spawn {
                            if let Material::Clips { suspended, .. } = &mut s.material {
                                *suspended = true;
                            }
                            return Err(format!("the device refused the next track — {e}"));
                        }
                        return Ok(serde_json::json!({
                            "done": false,
                            "state": serde_json::to_value(trial_state_of(s, false)).unwrap()
                        }));
                    }
                    _ => {
                        // Cancelled while we were away: tear down what we made.
                        TRIAL_OWNS_TRACK.store(false, std::sync::atomic::Ordering::Relaxed);
                        track_stop_inner();
                        return Err("session was cancelled".into());
                    }
                }
            }
        }
    }
    Ok(serde_json::json!({
        "done": false,
        "state": serde_json::to_value(trial_state_of(session, false)).unwrap()
    }))
}

/// Reveal the running score mid-session — recorded in provenance (TB-24).
#[tauri::command]
fn abx_reveal() -> Result<TrialState, String> {
    let mut guard = TRIAL.lock().unwrap();
    let session = guard.as_mut().ok_or("no session running")?;
    let at = session.answers.len();
    if !session.stats_viewed.contains(&at) {
        session.stats_viewed.push(at);
    }
    Ok(trial_state_of(session, true))
}

#[tauri::command]
fn abx_cancel() -> Result<(), String> {
    let mut guard = TRIAL.lock().unwrap();
    let was_clips = matches!(
        guard.as_ref().map(|s| &s.material),
        Some(Material::Clips { .. })
    );
    *guard = None;
    drop(guard);
    if was_clips {
        TRIAL_OWNS_TRACK.store(false, std::sync::atomic::Ordering::Relaxed);
        track_stop_inner();
    } else {
        let _ = apply_side("a");
        set_ab_side("a");
    }
    Ok(())
}

/// The honest exit after device loss or fatigue: persist what exists,
/// flagged `finishedEarly`. Refused before 4 answers — nothing worth keeping.
#[tauri::command]
fn trial_finish_early() -> Result<serde_json::Value, String> {
    let mut guard = TRIAL.lock().unwrap();
    let session = guard.as_mut().ok_or("no session running")?;
    if session.answers.len() < 4 {
        return Err(
            "fewer than 4 trials answered — cancel instead, there is nothing to record".into(),
        );
    }
    let result = trial_finalize(session, None, true)?;
    *guard = None;
    Ok(result)
}

/// After a device failure: respawn the current trial's clip and continue.
#[tauri::command]
async fn trial_resume(app: tauri::AppHandle) -> Result<TrialState, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<TrialState, String> {
        let mut guard = TRIAL.lock().unwrap();
        let session = guard.as_mut().ok_or("no session running")?;
        let (clip, rate) = match &session.material {
            Material::Clips {
                plan,
                rate,
                suspended: true,
                ..
            } => {
                let idx = session.answers.len().min(plan.len() - 1);
                (plan[idx].clone(), *rate)
            }
            Material::Clips { .. } => return Err("the session is already running".into()),
            Material::System => return Err("nothing to resume".into()),
        };
        let new_rate =
            fletcher_core::playback::probe_rate(fletcher_core::playback::OutputMode::Exclusive)
                .map_err(|_| "the device still refuses exclusive mode".to_string())?;
        if new_rate != rate {
            return Err(
                "the device now runs at a different rate — finish early or cancel this session"
                    .into(),
            );
        }
        let opening = session.audition.clone();
        let hear_a = {
            // The audition string survives suspension; re-resolve it.
            let t = if opening.is_empty() { "x" } else { &opening };
            trial_hear_a(session, t)
        };
        let sid = session.id.clone();
        let (a, b, lm) = (session.a.clone(), session.b.clone(), session.level_matched);
        spawn_trial_clip(
            &app,
            &sid,
            TrialChains {
                a: &a,
                b: &b,
                level_matched: lm,
            },
            &clip,
            rate,
            hear_a,
        )?;
        if let Material::Clips { suspended, .. } = &mut session.material {
            *suspended = false;
        }
        Ok(trial_state_of(session, false))
    })
    .await
    .map_err(|e| e.to_string())?
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

/// Transport keys for the scope pop-outs (Q-20 law: satellites get no DOM
/// keys, so these are focus-scoped OS shortcuts). Space toggles directly in
/// Rust; C goes to the focused scope window (it owns the hover position);
/// I/O go to main (it owns the region).
fn set_scope_shortcuts(app: &tauri::AppHandle, label: &str, enable: bool) {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
    let gs = app.global_shortcut();
    for key in ["space", "c", "i", "o", "escape"] {
        if enable {
            let label = label.to_string();
            let _ = gs.on_shortcut(key, move |app, _shortcut, event| {
                use tauri::Emitter;
                let pressed = event.state == ShortcutState::Pressed;
                match key {
                    "space" => {
                        if pressed {
                            track_toggle_inner();
                        }
                    }
                    // C carries press AND release — held-C is the audible scrub.
                    "c" => {
                        let _ = app.emit_to(
                            label.as_str(),
                            "scope-key",
                            serde_json::json!({ "key": "c", "state": if pressed { "down" } else { "up" } }),
                        );
                    }
                    _ => {
                        if pressed {
                            let _ = app.emit_to(
                                "main",
                                "scope-key",
                                serde_json::json!({ "key": key, "state": "down" }),
                            );
                        }
                    }
                }
            });
        } else {
            let _ = gs.unregister(key);
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
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
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
                tauri::WindowEvent::Focused(focused) if window.label().starts_with("scope-") => {
                    set_scope_shortcuts(window.app_handle(), window.label(), *focused);
                }
                tauri::WindowEvent::Destroyed if window.label().starts_with("scope-") => {
                    set_scope_shortcuts(window.app_handle(), window.label(), false);
                    // The main window puts the pane back inline.
                    use tauri::Emitter;
                    let _ = window.app_handle().emit_to(
                        "main",
                        "scope-closed",
                        window.label().to_string(),
                    );
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
            trial_start,
            trial_finish_early,
            trial_resume,
            preset_chain,
            autoeq_search,
            autoeq_import,
            preset_rename,
            parse_filters,
            chain_curves,
            preview_chain,
            preset_create_from_chain,
            settings_state,
            set_reference_db,
            set_level_matching,
            engine_test_tone,
            calibration_noise,
            tools_state,
            tools_install,
            library_state,
            track_import,
            track_delete,
            studio_state,
            set_studio_mode,
            track_play,
            track_toggle,
            track_seek,
            track_scrub,
            track_scrub_params,
            signal_create,
            signal_update,
            open_data_dir,
            signal_preview,
            signal_validate,
            track_import_url,
            track_stop,
            track_waveform,
            track_samples,
            track_spectrogram,
            track_fft,
            track_status,
            track_loop,
            clip_create,
            clip_update,
            clip_tag,
            clip_delete,
            battery_state,
            battery_create,
            battery_update,
            battery_set_clips,
            battery_delete,
            history_save,
            history_load,
            history_export,
            history_import
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
