//! Fletcher's Tauri shell: thin command layer over fletcher-core (ADR-0001/0005 —
//! the UI draws, the core thinks). DTOs live here so the core stays serde-free.

use fletcher_core::config::{render_fletcher_file, ChainFilter, ConfigDoc, FilterKind, Parsed};
use fletcher_core::dsp::{chain_response_db, log_freqs, loudness_db, FilterSpec};
use fletcher_core::presets::{sanitize_name, PresetStore};
use fletcher_core::{apo, devices, dsp, fsx, rew};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

mod analysis;
mod engine;
mod profile;
mod tools;
mod updates;

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

/// Output trim bounds. The trim is the ONE lever that can push Fletcher's
/// written preamp above 0 dB — `reference_db` is capped at 0 by design, so
/// before this existed there was no way to answer "it's too quiet" from
/// inside the app when attenuation lived in a line Fletcher doesn't own
/// (ADR-0007) — e.g. a hand-added `Preamp:` in config.txt. Hit live
/// 2026-08-29, TB-31.
const TRIM_MIN_DB: f64 = -20.0;
const TRIM_MAX_DB: f64 = 12.0;
/// Total written preamp bounds. The ceiling is the trim's, so a user
/// compensating external attenuation can reach it, and no further. The floor is
/// a sanity bound, NOT a safeguard: no finite floor covers every accepted chain
/// (±30 dB per filter, any number of filters). The clip ceiling is applied after
/// it, and a plan the floor would distort is reported as unmatched (review F5).
const PREAMP_MIN_DB: f64 = -120.0;
const PREAMP_MAX_DB: f64 = TRIM_MAX_DB;
/// A level rise bigger than this is never written in one go: it's stepped up
/// 1 dB at a time so nothing ever surprises the listener (TB-20, ADR-0016).
const MAX_JUMP_DB: f64 = 3.0;

/// A user-set output trim added to every matched preamp, on every side.
///
/// Deliberately an OFFSET, not an override: `matched_preamp` is what makes
/// ADR-0003's level matching true, so replacing it would make every A/B a
/// louder-vs-quieter comparison. A constant added to both sides cancels in
/// any comparison — the trim moves absolute loudness and leaves every
/// *relative* guarantee (and `shortfall`) exactly as it was.
fn preamp_trim_db() -> f64 {
    read_state()
        .get("preampTrimDb")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0)
        .clamp(TRIM_MIN_DB, TRIM_MAX_DB)
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
/// Where every EQ curve starts: 10 Hz, the lowest Fc a filter can have, so
/// nothing a filter does is off the left edge of a graph.
const CURVE_FMIN: f64 = 10.0;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EqState {
    device_name: Option<String>,
    /// Whether APO processes the default device at all: "active" | "missing" |
    /// "disabled" | "unknown" (TB-32). Anything but "active" means every edit
    /// below is inaudible, and the UI must say so.
    apo_on_device: &'static str,
    /// APO was registered on this device during this run. APO 1.4.2 takes effect
    /// immediately (verified 2026-10-26); older versions may need a restart.
    apo_reboot_pending: bool,
    /// Every Preamp line APO applies, summed (Fletcher's + outside ones).
    preamp_db: f64,
    /// Fletcher's own auto preamp alone (fletcher.txt). The UI shows this as
    /// "auto preamp" and names the outside part separately (TB-31).
    own_preamp_db: f64,
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
    let mut own_preamp_db = 0.0;
    // q stays an Option: an unwritten Q means something specific to APO (TB-34).
    let mut filters: Vec<(bool, FilterKind, f64, f64, Option<f64>, String)> = Vec::new();
    let mut source_files = Vec::new();
    for (name, doc) in &docs {
        let mut contributed = false;
        for d in doc.directives() {
            match d {
                Parsed::Preamp { db } => {
                    preamp_db += db;
                    if name.eq_ignore_ascii_case("fletcher.txt") {
                        own_preamp_db += db;
                    }
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
                        *q,
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

    let freqs = log_freqs(CURVE_FMIN, 20000.0, CURVE_POINTS);
    // Every line is drawn and summed as the filter APO actually plays (TB-34);
    // one APO rejects (a Q-less PK) contributes nothing, as in APO.
    let resolved: Vec<Option<FilterSpec>> = filters
        .iter()
        .map(|&(_, kind, fc_hz, gain_db, q, _)| dsp::apo_resolve(kind, fc_hz, gain_db, q))
        .collect();
    let specs: Vec<FilterSpec> = filters
        .iter()
        .zip(&resolved)
        .filter(|(f, _)| f.0)
        .filter_map(|(_, r)| *r)
        .collect();
    let sum_db = chain_response_db(&specs, preamp_db, FS, &freqs);

    let filter_dtos = filters
        .iter()
        .zip(&resolved)
        .map(|((enabled, kind, fc_hz, gain_db, q, source), r)| {
            let biquad = r.map_or(dsp::Biquad::IDENTITY, |r| r.biquad(FS));
            EqFilter {
                enabled: *enabled,
                kind: kind.code(),
                fc_hz: *fc_hz,
                gain_db: *gain_db,
                // An unwritten Q shows the one APO uses (30 for a bare NO).
                q: q.or(r.map(|r| r.q))
                    .unwrap_or(std::f64::consts::FRAC_1_SQRT_2),
                response_db: freqs.iter().map(|&f| biquad.magnitude_db(f, FS)).collect(),
                source_file: source.clone(),
            }
        })
        .collect();

    let includes = docs.iter().skip(1).map(|(n, _)| n.clone()).collect();
    let (apo_on_device, apo_reboot_pending) = default_device_apo();
    Ok(EqState {
        device_name: devices::default_render_device_name(),
        apo_on_device,
        apo_reboot_pending,
        preamp_db,
        own_preamp_db,
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
                // Below Nyquist: a filter AT Nyquist has all-zero coefficients
                // and silences everything.
                fc_hz: f.fc_hz.clamp(10.0, 0.49 * FS),
                gain_db: f.gain_db.clamp(-30.0, 30.0),
                q: f.q.clamp(0.01, 100.0),
            })
        })
        .collect()
}

/// The single writer of fletcher.txt. Writes the file, then guarantees
/// config.txt includes it (ADR-0007) — the wiring is lost whenever APO is
/// reinstalled/reset or another tool rewrites config.txt, and without it every
/// Fletcher write is silently inaudible and invisible. Only appends the one
/// line; backs config.txt up once before the first touch.
fn write_fletcher_file(config_dir: &std::path::Path, text: &str) -> Result<(), String> {
    fsx::write_atomic(&config_dir.join("fletcher.txt"), text).map_err(|e| e.to_string())?;
    let root_path = config_dir.join("config.txt");
    let root_text = std::fs::read_to_string(&root_path).unwrap_or_default();
    let mut root = ConfigDoc::parse(&root_text);
    if root.ensure_include("fletcher.txt") {
        let backup = config_dir.join("config.txt.before-fletcher.bak");
        if !backup.exists() {
            fsx::write_atomic(&backup, &root_text).map_err(|e| e.to_string())?;
        }
        fsx::write_atomic(&root_path, &root.to_string())
            .map_err(|e| format!("cannot wire fletcher.txt into config.txt: {e}"))?;
    }
    Ok(())
}

/// Loudness last written to fletcher.txt (the chain's loudness + its preamp),
/// for the rise limit. `None` until this run's first write.
static LAST_WRITTEN_LEVEL: std::sync::Mutex<Option<f64>> = std::sync::Mutex::new(None);

/// THE way Fletcher's chain reaches fletcher.txt. Never raises loudness by more
/// than `MAX_JUMP_DB` in one write (TB-20, ADR-0016): a bigger rise becomes a
/// short fade-in (`fade_steps`, ~80 ms apart, which APO hot-reloads). That
/// covers every path at once: a typed reference or trim, turning matching off,
/// a preset switch, leaving a paired comparison, ending a trial. Drops apply
/// immediately. The lock also serializes writers, so a fade is never interleaved.
fn write_chain_safely(chain: &[ChainFilter], preamp: f64) -> Result<(), String> {
    if !preamp.is_finite() {
        return Err("refusing to write a non-finite preamp".into());
    }
    let install = apo::detect().map_err(|e| e.to_string())?;
    let level = loudness_db(&specs_of(chain), FS) + preamp;
    let mut last = LAST_WRITTEN_LEVEL.lock().unwrap();
    // The first write of a run (whatever the last session left playing)
    // fades in from 12 dB below. A silent chain's −∞ loudness counts the same.
    let prev = last.filter(|p| p.is_finite()).unwrap_or(level - 12.0);
    for below in fade_steps(level - prev) {
        let step = round_tenth(preamp - below);
        write_fletcher_file(&install.config_path, &render_fletcher_file(step, chain))?;
        std::thread::sleep(std::time::Duration::from_millis(80));
    }
    write_fletcher_file(&install.config_path, &render_fletcher_file(preamp, chain))?;
    *last = Some(level);
    Ok(())
}

/// How far below the final level each fade write sits, for a rise of `rise`
/// dB. Nothing for a rise within `MAX_JUMP_DB`. Otherwise at most 12 writes
/// (~1 s), each rising by the same step of 1 to 3 dB. A rise over 36 dB starts
/// 36 dB below: from something that quiet, that's still a fade-in.
fn fade_steps(rise: f64) -> Vec<f64> {
    if rise.is_nan() || rise <= MAX_JUMP_DB {
        return Vec::new();
    }
    let rise = rise.min(36.0);
    let step = (rise / 12.0).max(1.0);
    let mut out = Vec::new();
    let mut below = rise - step;
    while below > 0.05 {
        out.push(below);
        below -= step;
    }
    out
}

/// Refuse a direct chain write while a blind test runs: it would change what's
/// playing under the trial and record votes against the wrong sound (TB-26).
fn refuse_mid_trial() -> Result<(), String> {
    if TRIAL.lock().unwrap().is_some() {
        return Err("a blind test is running — finish or cancel it first".into());
    }
    Ok(())
}

/// Replace Fletcher's own chain (fletcher.txt) with the given filters.
/// Auto-preamp is computed from the summed response peak (TB-06), the file
/// is written atomically (TB-11), and APO hot-reloads it. Returns the fresh
/// merged state.
#[tauri::command]
fn set_fletcher_chain(filters: Vec<FilterIn>) -> Result<EqState, String> {
    refuse_mid_trial()?;
    let chain = chain_of(&filters)?;
    let specs = specs_of(&chain);
    write_chain_safely(&chain, matched_preamp_full(&specs).0)?;
    push_chain_to_track(&chain);

    // Edits persist into the active preset, so switching away and back keeps them.
    if let Some(name) = active_preset() {
        let _ = store()?.save(&name, preset_preamp(&specs), &chain);
    }
    // Editing implies listening to the chain: land on A.
    set_ab_side("a");

    eq_state()
}

/// Rewrite fletcher.txt from a chain (with fresh auto-preamp). Also feeds a
/// running track session — the tuning loop and the engine share one truth.
fn activate_chain(chain: &[ChainFilter]) -> Result<(), String> {
    activate_chain_at(chain, matched_preamp_full(&specs_of(chain)).0)
}

/// `activate_chain` at an explicit preamp: comparisons pass the PAIRED level
/// from `matched_pair`, so both sides of an A/B or a blind test match (TB-08).
fn activate_chain_at(chain: &[ChainFilter], preamp: f64) -> Result<(), String> {
    write_chain_safely(chain, preamp)?;
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
    refuse_mid_trial()?;
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
    refuse_mid_trial()?;
    let name = sanitize_name(&name).ok_or("invalid preset name")?;
    let st = store()?;
    if st.exists(&name) {
        return Err(format!("a preset named {name:?} already exists"));
    }
    let chain = if from_live { live_chain()? } else { Vec::new() };
    let specs = specs_of(&chain);
    st.save(&name, preset_preamp(&specs), &chain)
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
            } => chain.extend(own_filter(*enabled, *kind, *fc_hz, *gain_db, *q)),
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

fn round_tenth(db: f64) -> f64 {
    (db * 10.0).round() / 10.0
}

/// What Equalizer APO applies on the playback path besides fletcher.txt, read
/// simply (ADR-0016): config.txt and the files it includes directly. Fletcher
/// does not model other tools' processing; it only asks whether the outside
/// path is PLAIN, i.e. nothing but `Preamp:`, `Include:` and comments, all
/// readable. Only then do outside cuts provably apply to this device and count
/// as headroom. Anything else (Peace's filters, GraphicEQ, Device/Channel
/// scopes, nested includes, a missing file) means "not provable", and Fletcher
/// errs quiet.
#[derive(Default, Debug, PartialEq)]
struct OutsidePath {
    /// Sum of outside Preamp cuts (≤ 0), whatever their scope.
    cut_db: f64,
    /// Sum of outside Preamp boosts (≥ 0), whatever their scope.
    boost_db: f64,
    plain: bool,
    /// A file on the path couldn't be read: its effect is unknown.
    unreadable: bool,
}

impl OutsidePath {
    /// For the clip ceiling: boosts always count; cuts only when provable.
    fn ceiling_preamp_db(&self) -> f64 {
        self.boost_db + if self.plain { self.cut_db } else { 0.0 }
    }

    /// For exclusive playback, which must recreate the shared path's level and
    /// must never come out louder: every cut counts, whatever its scope; boosts
    /// don't; an unreadable file costs a 10 dB pad.
    fn exclusive_preamp_db(&self) -> f64 {
        self.cut_db + if self.unreadable { -10.0 } else { 0.0 }
    }
}

fn outside_path() -> OutsidePath {
    match apo::detect() {
        Ok(install) => outside_path_in(&install.config_path),
        Err(_) => OutsidePath::default(),
    }
}

fn outside_path_in(dir: &std::path::Path) -> OutsidePath {
    let mut out = OutsidePath {
        plain: true,
        ..OutsidePath::default()
    };
    read_outside(&dir.join("config.txt"), dir, true, &mut out);
    out
}

fn read_outside(file: &std::path::Path, dir: &std::path::Path, top: bool, out: &mut OutsidePath) {
    let Ok(text) = std::fs::read_to_string(file) else {
        out.unreadable = true;
        out.plain = false;
        return;
    };
    for line in text.lines().map(str::trim) {
        let key = line
            .split(':')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if !(line.is_empty() || line.starts_with('#') || key == "preamp" || key == "include") {
            out.plain = false;
        }
    }
    for d in ConfigDoc::parse(&text).directives() {
        match d {
            Parsed::Preamp { db } if *db > 0.0 => out.boost_db += db,
            Parsed::Preamp { db } => out.cut_db += db,
            Parsed::Include { path } if path.eq_ignore_ascii_case("fletcher.txt") => {}
            // Nested includes aren't followed: not plain.
            Parsed::Include { path } if top => read_outside(&dir.join(path), dir, false, out),
            Parsed::Include { .. } => out.plain = false,
            _ => {}
        }
    }
}

/// One chain's level plan (ADR-0016). Returns (preamp to write, shortfall).
///
/// - target = reference + `trim`: the loudness everything lands at.
/// - Level matching on: the preamp that puts the chain's loudness (≈ LUFS on
///   pink noise) at the target. Off: every chain gets the target preamp, so
///   presets keep their own loudness relative to Flat and Flat stays at the
///   reference (never a jump).
/// - Never above the clip ceiling (see `dsp::plan_level`).
/// - shortfall > 0: held below the target by the ceiling. < 0: infeasible.
fn level_plan(specs: &[FilterSpec], trim: f64, outside: &OutsidePath) -> (f64, f64) {
    dsp::plan_level(
        specs,
        outside.ceiling_preamp_db(),
        FS,
        reference_db().min(0.0) + trim,
        level_matching(),
        (PREAMP_MIN_DB, PREAMP_MAX_DB),
    )
}

/// The plan for what plays through Equalizer APO: this machine's trim and the
/// outside path.
fn matched_preamp_full(specs: &[FilterSpec]) -> (f64, f64) {
    level_plan(specs, preamp_trim_db(), &outside_path())
}

/// The plan for the track engine's exclusive path, which bypasses APO
/// entirely: nothing outside applies there, and the listening level is applied
/// as stream gain instead (`exclusive_listening_db`), so the bus stays clip-safe.
fn engine_preamp_full(specs: &[FilterSpec]) -> (f64, f64) {
    level_plan(specs, 0.0, &OutsidePath::default())
}

/// Preamps for two chains that will be COMPARED (TB-08 v2, ADR-0016). Each side
/// is planned on its own; if the ceiling holds either one short, BOTH come down
/// by the larger shortfall, so they play at the same loudness. Returns
/// (preamp_a, preamp_b, dB below the target, matched). `matched` is false only
/// when equality is impossible, and then the UI must say "unmatched" (F5).
fn pair_from(plan_a: (f64, f64), plan_b: (f64, f64)) -> (f64, f64, f64, bool) {
    dsp::pair_levels(plan_a, plan_b, (PREAMP_MIN_DB, PREAMP_MAX_DB))
}

/// A pair played through Equalizer APO (system A/B, system-audio trials, previews).
fn matched_pair(a: &[FilterSpec], b: &[FilterSpec]) -> (f64, f64, f64, bool) {
    let (outside, trim) = (outside_path(), preamp_trim_db());
    pair_from(level_plan(a, trim, &outside), level_plan(b, trim, &outside))
}

/// A pair played by the track engine (clip trials).
fn engine_pair(a: &[FilterSpec], b: &[FilterSpec]) -> (f64, f64, f64, bool) {
    pair_from(engine_preamp_full(a), engine_preamp_full(b))
}

/// The preamp a PRESET stores. Deliberately untrimmed (ADR-0014) and without
/// this machine's outside path: both describe this machine's playback, not the
/// tuning. Baking them in would make an exported preset arrive at someone
/// else's ears offset by a number that only ever meant something here.
fn preset_preamp(specs: &[FilterSpec]) -> f64 {
    engine_preamp_full(specs).0
}

/// Clip Studio brings every song to this loudness before anything else, in
/// BOTH play methods (Flat and EQ), so Flat ↔ EQ and song ↔ song stay at one
/// fixed level (ADR-0016; the owner's choice for review F8). −16 LUFS is the
/// common streaming level: nearly every modern master comes DOWN to it.
/// Attenuate-only, so a quieter master stays a little quieter, but it gets the
/// same gain in both methods, so the comparison is unaffected.
const SONG_NORM_LUFS: f64 = -16.0;

/// The Windows volume exclusive mode skips (TB-37): 0 dB on devices whose
/// volume is in hardware (it applies to exclusive streams too, so recreating it
/// would apply it twice). If the device can't be read, assume a quiet −20 dB
/// rather than full volume.
fn bypassed_volume_db() -> f64 {
    devices::default_render_bypassed_volume_db().unwrap_or(-20.0)
}

/// What the exclusive engine adds after song normalization to land where the
/// shared path would put Flat at `flat_preamp_db`: that preamp, the outside
/// Preamp cuts, and the Windows volume exclusive mode skips. Errs quiet
/// (`exclusive_preamp_db`). Always ≤ 0 dB. Only for EXCLUSIVE output: a shared
/// stream gets all of these from APO and Windows already (review F2). Sampled
/// when playback starts, so a Windows volume change mid-song takes effect from
/// the next play, as with any exclusive-mode player.
fn exclusive_listening_db(flat_preamp_db: f64) -> f64 {
    (flat_preamp_db + outside_path().exclusive_preamp_db() + bypassed_volume_db()).min(0.0)
}

/// Flat's preamp as `apply_side` writes it for side B: paired with the active
/// chain, so never above the level Flat plays at in the A/B (follow-up R4).
fn compared_flat_preamp_db() -> f64 {
    matched_pair(&specs_of(&active_chain()), &[]).1
}

/// Clip Studio's measured A/B (ADR-0016): the playing session it was measured
/// in, and the chain it was measured with.
type StudioMeasured = (std::sync::Weak<engine::TrackShared>, Vec<ChainFilter>);
static STUDIO_MEASURED: std::sync::Mutex<Option<StudioMeasured>> = std::sync::Mutex::new(None);

/// The playing Clip Studio session when APO does the EQ (Through-EQ): the one
/// case where there's a song to measure and a raw stream to correct.
fn studio_through_eq() -> Option<(i64, std::sync::Arc<engine::TrackShared>)> {
    if trial_owns_track() {
        return None;
    }
    let guard = TRACK.lock().unwrap();
    let s = guard.as_ref()?;
    matches!(s.mode, fletcher_core::playback::OutputMode::Shared)
        .then(|| (s.track_id, s.shared.clone()))
}

/// We have the actual audio, so don't estimate: measure A (chain at `pa`) and
/// B (Flat at `pb`) on what's being heard, i.e. the loop when one is on, else
/// the 20 s around the playhead, read from the decode cache. Returns
/// LUFS(A) − LUFS(B), or None (too short, silent, no cache) and the estimate
/// stands. ~40 ms in a release build.
fn studio_measured_diff(
    id: i64,
    shared: &engine::TrackShared,
    specs: &[FilterSpec],
    pa: f64,
    pb: f64,
) -> Option<f64> {
    use std::io::{Read, Seek, SeekFrom};
    use std::sync::atomic::Ordering::Relaxed;
    let (rate, total) = (shared.rate, shared.total_frames);
    let window = 20 * rate as u64;
    let (loop_a, loop_b) = (
        shared.loop_a.load(Relaxed),
        shared.loop_b.load(Relaxed).min(total),
    );
    let (start, len) = if shared.loop_on.load(Relaxed) && loop_b > loop_a {
        (loop_a, (loop_b - loop_a).min(window))
    } else {
        let len = window.min(total);
        (
            shared
                .pos
                .load(Relaxed)
                .saturating_sub(len / 2)
                .min(total - len),
            len,
        )
    };
    let path = data_dir()
        .join("cache")
        .join("pcm")
        .join(format!("{id}-{rate}.f32"));
    let mut file = std::fs::File::open(path).ok()?;
    file.seek(SeekFrom::Start(start * 8)).ok()?;
    let mut bytes = vec![0u8; (len * 8) as usize];
    file.read_exact(&mut bytes).ok()?;
    let pcm: Vec<f32> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect();
    let (la, _) = engine::lufs_peak_through(&pcm, rate, specs, pa).ok()?;
    let (lb, _) = engine::lufs_peak_through(&pcm, rate, &[], pb).ok()?;
    let diff = la - lb;
    diff.is_finite().then_some(diff)
}

/// Whether the A/B now playing was measured: in this very Clip Studio session
/// (not a replay of the same song), for this chain (not since an edit).
fn studio_measured_now(chain: &[ChainFilter]) -> bool {
    let Some((_, playing)) = studio_through_eq() else {
        return false;
    };
    let guard = STUDIO_MEASURED.lock().unwrap();
    guard.as_ref().is_some_and(|(session, measured)| {
        session
            .upgrade()
            .is_some_and(|s| std::sync::Arc::ptr_eq(&s, &playing))
            && measured.as_slice() == chain
    })
}

/// The measured correction for the side now playing: the louder side comes
/// down, nothing is raised (≤ 0 dB).
fn studio_side_trim_db(diff_a_minus_b: f64, side: &str) -> f64 {
    match side {
        "b" => diff_a_minus_b.min(0.0),
        _ => (-diff_a_minus_b).min(0.0),
    }
}

/// Write fletcher.txt for the given side: A = the active chain; B = flat,
/// level-matched to the chain's average loudness. Both sides use the PAIRED
/// levels, so a boost that hits the clip cap brings Flat down with it instead
/// of leaving it louder (TB-08 v2).
///
/// While Clip Studio plays a song Through-EQ, the flip is also MEASURED on
/// that song and the correction goes on Clip Studio's own stream, never into
/// fletcher.txt: it affects only the song, ends with it, and leaves the
/// system-wide levels (and the fade limiter) on the plain estimate.
fn apply_side(side: &str) -> Result<(), String> {
    let chain = active_chain();
    let specs = specs_of(&chain);
    let (pa, pb, _, matched) = matched_pair(&specs, &[]);
    match side {
        // Flat goes to the file only; a running track keeps the active chain.
        "b" => write_chain_safely(&[], pb)?,
        _ => activate_chain_at(&chain, pa)?,
    }
    *STUDIO_MEASURED.lock().unwrap() = None;
    if let Some((id, shared)) = studio_through_eq() {
        let diff = (matched && level_matching())
            .then(|| studio_measured_diff(id, &shared, &specs, pa, pb))
            .flatten();
        let trim = diff.map_or(0.0, |d| studio_side_trim_db(d, side));
        shared.raw_gain_bits.store(
            10f64.powf(trim / 20.0).to_bits(),
            std::sync::atomic::Ordering::Relaxed,
        );
        if diff.is_some() {
            *STUDIO_MEASURED.lock().unwrap() = Some((std::sync::Arc::downgrade(&shared), chain));
        }
    }
    Ok(())
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
            } => chain.extend(own_filter(*enabled, *kind, *fc_hz, *gain_db, *q)),
            _ => {}
        }
    }
    if chain.is_empty() {
        return Err("that AutoEQ entry contained no parametric filters".into());
    }
    import_as_preset(name, preamp, &chain).map(|_| ())
}

/// Adopt a filter line from a file Fletcher doesn't own into Fletcher's chain,
/// keeping its SOUND (TB-34). Fletcher always writes a Q, and APO reads a
/// written Q differently from an unwritten one (a bare `NO` is Q 30; a bare
/// `LS` is slope S 0.9 at center, while `LS … Q` is a corner shelf). So a line
/// without a Q is resolved to the identical fully written filter first. A line
/// with a Q is kept verbatim, since Fletcher rewrites it the same way. `None` =
/// APO plays nothing for it (a Q-less PK), so there's nothing to adopt.
fn own_filter(
    enabled: bool,
    kind: FilterKind,
    fc_hz: Option<f64>,
    gain_db: Option<f64>,
    q: Option<f64>,
) -> Option<ChainFilter> {
    let fc_hz = fc_hz.unwrap_or(1000.0);
    let gain_db = gain_db.unwrap_or(0.0);
    match q {
        Some(q) => Some(ChainFilter {
            enabled,
            kind,
            fc_hz,
            gain_db,
            q,
        }),
        None => dsp::apo_resolve(kind, fc_hz, gain_db, None).map(|r| ChainFilter {
            enabled,
            kind: r.kind,
            fc_hz: r.fc_hz,
            gain_db: r.gain_db,
            q: r.q,
        }),
    }
}

/// Save an imported chain as a NEW preset (auto-suffixed, never overwriting)
/// and make it the active, audible one on side A. Returns the preset's name.
fn import_as_preset(name: &str, preamp: f64, chain: &[ChainFilter]) -> Result<String, String> {
    let st = store()?;
    let base = sanitize_name(name).ok_or("unusable preset name")?;
    let mut preset_name = base.clone();
    let mut n = 2;
    while st.exists(&preset_name) {
        preset_name = format!("{base} {n}");
        n += 1;
    }
    st.save(&preset_name, preamp, chain)
        .map_err(|e| e.to_string())?;
    activate_chain(chain)?;
    set_active_preset(Some(&preset_name));
    set_ab_side("a");
    Ok(preset_name)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RewSkippedDto {
    line: usize,
    text: String,
    reason: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RewImportResult {
    preset: String,
    imported: usize,
    /// Lines that couldn't be reproduced exactly — shown to the user, never
    /// silently dropped (issue #1).
    skipped: Vec<RewSkippedDto>,
    state: EqState,
}

/// Import a REW "Export filter settings as text" file as a new preset
/// (GitHub issue #1). Interpreted exactly as Equalizer APO would play it.
#[tauri::command]
fn rew_import(path: String) -> Result<RewImportResult, String> {
    let bytes = std::fs::read(&path).map_err(|e| format!("couldn't read {path}: {e}"))?;
    if bytes.starts_with(&[0xAC, 0xED]) {
        // Java serialization magic: REW's own .req "Save filter settings" file.
        return Err("that's REW's own save file (.req), which only REW can read. In REW's EQ window use File › Export › Export filter settings as text, then import that .txt".into());
    }
    let text = String::from_utf8_lossy(&bytes);
    let parsed = rew::parse_rew(&text);
    if parsed.filters.is_empty() {
        return Err(if parsed.skipped.is_empty() {
            "no REW filters found in that file — it needs lines like “Filter 1: ON PK Fc 63 Hz Gain -4 dB Q 2”".into()
        } else {
            format!(
                "none of the {} filter(s) could be imported exactly: {}",
                parsed.skipped.len(),
                parsed.skipped[0].reason
            )
        });
    }
    let stem = std::path::Path::new(&path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "REW import".into());
    let preset = import_as_preset(
        &stem,
        preset_preamp(&specs_of(&parsed.filters)),
        &parsed.filters,
    )?;
    Ok(RewImportResult {
        preset,
        imported: parsed.filters.len(),
        skipped: parsed
            .skipped
            .into_iter()
            .map(|s| RewSkippedDto {
                line: s.line,
                text: s.text,
                reason: s.reason,
            })
            .collect(),
        state: eq_state()?,
    })
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
/// preset's worth of lines works). Non-filter lines are ignored. A pasted REW
/// export goes through the REW reader, so it lands exactly as APO would play it.
#[tauri::command]
fn parse_filters(text: String) -> Vec<PastedFilter> {
    if rew::looks_like_rew(&text) {
        return rew::parse_rew(&text)
            .filters
            .into_iter()
            .map(|f| PastedFilter {
                enabled: f.enabled,
                kind: f.kind.code(),
                fc_hz: f.fc_hz,
                gain_db: f.gain_db,
                q: f.q,
            })
            .collect();
    }
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
            } => own_filter(*enabled, *kind, *fc_hz, *gain_db, *q).map(|f| PastedFilter {
                enabled: f.enabled,
                kind: f.kind.code(),
                fc_hz: f.fc_hz,
                gain_db: f.gain_db,
                q: f.q,
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
    /// How far the clip ceiling kept this chain below the target. Adding it to
    /// the audible curve makes any two curves' difference the PAIRED difference
    /// (ADR-0016): (pa + sa) − (pb + sb) is exactly what `matched_pair` plays.
    shortfall_db: f64,
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
    let freqs = log_freqs(CURVE_FMIN, 20000.0, CURVE_POINTS);
    let curves = chains
        .iter()
        .map(|filters| {
            let specs = specs_of(&chain_of(filters)?);
            let (preamp, shortfall) = matched_preamp_full(&specs);
            Ok(ChainCurve {
                response_db: chain_response_db(&specs, 0.0, FS, &freqs),
                matched_preamp_db: preamp,
                shortfall_db: shortfall,
            })
        })
        .collect::<Result<_, String>>()?;
    Ok(ChainCurves { freqs, curves })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FilterCurves {
    freqs: Vec<f64>,
    curves: Vec<Vec<f64>>,
}

/// Each filter's own magnitude response, nothing else: no level planning and no
/// config reads, so it's cheap enough to call on every slider move (the filter
/// editor's live plot and type thumbnails, Q-27). Same math as the graph.
#[tauri::command]
fn filter_responses(filters: Vec<FilterIn>) -> Result<FilterCurves, String> {
    let freqs = log_freqs(CURVE_FMIN, 20000.0, CURVE_POINTS);
    let curves = specs_of(&chain_of(&filters)?)
        .iter()
        .map(|spec| chain_response_db(std::slice::from_ref(spec), 0.0, FS, &freqs))
        .collect();
    Ok(FilterCurves { freqs, curves })
}

/// Level-matched, non-destructive audition of an arbitrary chain (the history
/// inspector's "listen"): writes fletcher.txt only — the active preset and the
/// A/B side are untouched, so the preview leaves no trace on disk state.
/// Refused mid-ABX: an outside write would break the blinding.
///
/// `partner` is what this preview is being compared against (the other node in
/// the inspector, the other side of a finished blind test). With it, both play
/// at the PAIRED level (ADR-0016), so flipping between them is never
/// louder-vs-quieter, the same guarantee A/B and the trials give.
///
/// Returns whether it IS matched: false when equality is impossible (an
/// infeasible chain), so the UI can say so instead of implying a match
/// (follow-up R6).
#[tauri::command]
fn preview_chain(filters: Vec<FilterIn>, partner: Option<Vec<FilterIn>>) -> Result<bool, String> {
    if TRIAL.lock().unwrap().is_some() {
        return Err("a blind test is running — finish or cancel it first".into());
    }
    if profile_running() {
        return Err("a hearing-profile session is running — finish or cancel it first".into());
    }
    let chain = chain_of(&filters)?;
    let specs = specs_of(&chain);
    let (preamp, matched) = match partner {
        Some(p) => {
            let (pa, _, _, matched) = matched_pair(&specs, &specs_of(&chain_of(&p)?));
            (pa, matched)
        }
        None => {
            let (p, shortfall) = matched_preamp_full(&specs);
            (p, shortfall >= -0.05)
        }
    };
    activate_chain_at(&chain, preamp)?;
    Ok(matched)
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
    st.save(&name, preset_preamp(&specs_of(&chain)), &chain)
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
    if profile_running() {
        return Err("a hearing-profile session is running — finish or cancel it first".into());
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
    if profile_running() {
        return Err("a hearing-profile session is running — finish or cancel it first".into());
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
    // Calibrate against the reference itself: Flat at the reference (+ trim),
    // not whatever is active. A capped preset, or a paired Flat sitting below
    // the reference, would otherwise get the knob set several dB too loud
    // for everything afterwards (review F8).
    write_chain_safely(&[], matched_preamp_full(&[]).0)?;
    std::thread::spawn(move || {
        use fletcher_core::{playback, signal};
        let mut src = playback::SignalSource::new(
            |fs| vec![signal::Signal::new(signal::SignalKind::Pink, 0.2, fs, 42)],
            None,
        );
        let result = playback::play(playback::OutputMode::Shared, &mut src, 1.0, &stop, |_| {});
        // Put back what was playing, unless a session took over meanwhile.
        if TRIAL.lock().unwrap().is_none() && !profile_running() {
            let _ = apply_side(&ab_side());
        }
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
    // Migration: battery entries can be whole songs ('track') since ADR-0012+.
    let _ = conn.execute(
        "ALTER TABLE battery_clips ADD COLUMN kind TEXT NOT NULL DEFAULT 'clip'",
        [],
    );
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
        "DELETE FROM battery_clips WHERE clip_id = ?1 AND kind = 'clip'",
        rusqlite::params![id],
    )
    .map_err(|e| e.to_string())?;
    library_state_now()
}

// ---------------- batteries: the Lab's test material (phase 3) ----------------

/// One battery entry: a clip, or a whole song (kind "track").
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct BatteryItem {
    kind: String, // "clip" | "track"
    id: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BatteryRow {
    id: i64,
    name: String,
    note: Option<String>,
    items: Vec<BatteryItem>,
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
                items: Vec::new(),
                created_ms: r.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT battery_id, clip_id, kind FROM battery_clips ORDER BY battery_id, ord")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut map: std::collections::HashMap<i64, Vec<BatteryItem>> =
        std::collections::HashMap::new();
    for row in rows.flatten() {
        map.entry(row.0).or_default().push(BatteryItem {
            kind: row.2,
            id: row.1,
        });
    }
    for b in &mut batteries {
        if let Some(items) = map.remove(&b.id) {
            b.items = items;
        }
    }
    Ok(BatteryState { batteries })
}

#[tauri::command]
fn battery_state() -> Result<BatteryState, String> {
    battery_state_now()
}

#[tauri::command]
fn battery_create(name: String, items: Option<Vec<BatteryItem>>) -> Result<BatteryState, String> {
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
    if let Some(items) = items {
        battery_set_clips_inner(id, items)?;
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

/// Replace a battery's item list wholesale; vec order = play order. Items
/// whose clip/track no longer exists are dropped silently.
fn battery_set_clips_inner(id: i64, items: Vec<BatteryItem>) -> Result<(), String> {
    let mut conn = library_db()?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    tx.execute(
        "DELETE FROM battery_clips WHERE battery_id = ?1",
        rusqlite::params![id],
    )
    .map_err(|e| e.to_string())?;
    let mut seen = std::collections::HashSet::new();
    let mut ord = 0i64;
    for item in items {
        let kind = if item.kind == "track" {
            "track"
        } else {
            "clip"
        };
        if !seen.insert((kind, item.id)) {
            continue;
        }
        let table = if kind == "track" { "tracks" } else { "clips" };
        let exists: bool = tx
            .query_row(
                &format!("SELECT 1 FROM {table} WHERE id = ?1"),
                rusqlite::params![item.id],
                |_| Ok(true),
            )
            .unwrap_or(false);
        if !exists {
            continue;
        }
        tx.execute(
            "INSERT INTO battery_clips (battery_id, clip_id, kind, ord) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![id, item.id, kind, ord],
        )
        .map_err(|e| e.to_string())?;
        ord += 1;
    }
    tx.commit().map_err(|e| e.to_string())
}

#[tauri::command]
fn battery_set_clips(id: i64, items: Vec<BatteryItem>) -> Result<BatteryState, String> {
    battery_set_clips_inner(id, items)?;
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

// ---------------- the mic-free hearing profile (Q-19 / ADR-0013) ----------------

fn profiles_dir() -> PathBuf {
    data_dir().join("profiles")
}

fn profile_running() -> bool {
    profile::PROFILE.lock().unwrap().is_some()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProfileBandProgress {
    center_hz: f64,
    /// Min-step reversals banked across both staircases (0–12).
    banked: usize,
    state: &'static str, // "open" | "converged" | "railed" | "capped"
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProfileState {
    active: bool,
    phase: &'static str, // "levelSet" | "trial" | "done"
    paused: bool,
    headphone: String,
    level_dbfs: f64,
    level_warn: bool,
    presentations: u64,
    answered: u64,
    replays: u32,
    /// Total pair duration in ms — the UI animates interval tiles from this.
    pair_ms: u64,
    bands: Vec<ProfileBandProgress>,
}

fn profile_state_of(s: &profile::ProfileSession, paused: bool) -> ProfileState {
    use fletcher_core::stats::StaircaseEnd;
    let bands = s
        .bands
        .iter()
        .map(|b| {
            let banked: usize = b
                .stairs
                .iter()
                .map(|st| st.reversals().iter().filter(|(_, sp)| *sp <= 1.0).count())
                .sum();
            let ends: Vec<_> = b.stairs.iter().map(|st| st.done()).collect();
            let state = if ends.iter().all(|e| e.is_some()) {
                if ends.iter().any(|e| {
                    matches!(
                        e,
                        Some(StaircaseEnd::RailedHigh) | Some(StaircaseEnd::RailedLow)
                    )
                }) {
                    "railed"
                } else if ends.iter().any(|e| {
                    matches!(
                        e,
                        Some(StaircaseEnd::CappedOut) | Some(StaircaseEnd::Unconverged)
                    )
                }) {
                    "capped"
                } else {
                    "converged"
                }
            } else {
                "open"
            };
            ProfileBandProgress {
                center_hz: b.center_hz,
                banked,
                state,
            }
        })
        .collect();
    ProfileState {
        active: true,
        phase: if s.complete {
            "done"
        } else if s.anchor_dbfs.is_none() {
            "levelSet"
        } else {
            "trial"
        },
        paused,
        headphone: s.headphone.clone(),
        level_dbfs: s.level_dbfs(),
        level_warn: s.level_dbfs() > profile::LEVEL_WARN_DBFS,
        presentations: s.presentations,
        answered: s.answered,
        replays: s.current.as_ref().map(|c| c.replays).unwrap_or(0),
        pair_ms: ((profile::INTERVAL_S * 2.0 + profile::GAP_S) * 1000.0) as u64,
        bands,
    }
}

fn write_profile_record(record: &serde_json::Value) {
    let _ = std::fs::create_dir_all(profiles_dir());
    if let (Some(id), Ok(json)) = (
        record.get("id").and_then(|v| v.as_str()),
        serde_json::to_string_pretty(record),
    ) {
        let _ = fsx::write_atomic(&profiles_dir().join(format!("{id}.json")), &json);
    }
}

/// Start a hearing-profile session: exclusive-only (nothing may color the
/// measurement — not APO, not Fletcher, not Peace), opening in the level-set
/// step (anchor loops from −70 dBFS RMS; TB-20).
#[tauri::command]
async fn profile_start(app: tauri::AppHandle, headphone: String) -> Result<ProfileState, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<ProfileState, String> {
        if TRIAL.lock().unwrap().is_some() {
            return Err("a blind test is running — finish or cancel it first".into());
        }
        if TRACK.lock().unwrap().is_some() {
            return Err("stop track playback first — the profile needs the device exclusively".into());
        }
        if profile_running() {
            return Err("a hearing-profile session is already running".into());
        }
        stop_cal_noise();
        let rate = fletcher_core::playback::probe_rate(fletcher_core::playback::OutputMode::Exclusive)
            .map_err(|_| {
                "the hearing profile needs exclusive mode (nothing may color the measurement) — the device refused it"
                    .to_string()
            })?;
        let mut session = profile::ProfileSession::new(
            headphone.trim().to_string(),
            rate,
            String::new(),
        );
        session.shared.serial.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let app_err = app.clone();
        session.spawn_stream(move |e| {
            use tauri::Emitter;
            let _ = app_err.emit("profile-error", e);
        });
        let state = profile_state_of(&session, false);
        *profile::PROFILE.lock().unwrap() = Some(session);
        Ok(state)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn profile_set_level(dbfs: f64) -> Result<ProfileState, String> {
    let guard = profile::PROFILE.lock().unwrap();
    let s = guard.as_ref().ok_or("no profile session running")?;
    if s.anchor_dbfs.is_some() {
        return Err("the level is fixed for the whole session once accepted".into());
    }
    s.set_level_dbfs(dbfs);
    Ok(profile_state_of(s, false))
}

#[tauri::command]
fn profile_accept_level() -> Result<ProfileState, String> {
    let mut guard = profile::PROFILE.lock().unwrap();
    let s = guard.as_mut().ok_or("no profile session running")?;
    if s.anchor_dbfs.is_some() {
        return Err("the level is already accepted".into());
    }
    s.accept_level();
    write_profile_record(&s.record());
    Ok(profile_state_of(s, false))
}

/// One answer: "the second interval was louder" (true) or the first (false).
/// Journals, advances the staircase, persists the partial record (crash-safe
/// resume), and presents the next pair — or finishes the session.
#[tauri::command]
fn profile_answer(second_louder: bool) -> Result<serde_json::Value, String> {
    let mut guard = profile::PROFILE.lock().unwrap();
    let s = guard.as_mut().ok_or("no profile session running")?;
    s.answer(second_louder)?;
    let record = s.record();
    write_profile_record(&record);
    if s.complete {
        s.stop_stream();
        *guard = None;
        let mut result = record;
        if let Some(o) = result.as_object_mut() {
            o.remove("journal");
        }
        return Ok(serde_json::json!({ "done": true, "result": result }));
    }
    Ok(
        serde_json::json!({ "done": false, "state": serde_json::to_value(profile_state_of(s, false)).unwrap() }),
    )
}

/// Replay the identical pair (same seeds, same order). Recorded.
#[tauri::command]
fn profile_replay() -> Result<(), String> {
    let mut guard = profile::PROFILE.lock().unwrap();
    let s = guard.as_mut().ok_or("no profile session running")?;
    s.replay();
    Ok(())
}

/// Pause: release the device (the session stays; resume respawns).
#[tauri::command]
fn profile_pause() -> Result<ProfileState, String> {
    let guard = profile::PROFILE.lock().unwrap();
    let s = guard.as_ref().ok_or("no profile session running")?;
    s.stop_stream();
    Ok(profile_state_of(s, true))
}

#[tauri::command]
async fn profile_resume(app: tauri::AppHandle) -> Result<ProfileState, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<ProfileState, String> {
        let mut guard = profile::PROFILE.lock().unwrap();
        let s = guard.as_mut().ok_or("no profile session running")?;
        // Wait for the old stream to actually release before reopening.
        let done = s.stream_done.clone();
        let t0 = std::time::Instant::now();
        while !done.load(std::sync::atomic::Ordering::Relaxed)
            && t0.elapsed() < std::time::Duration::from_secs(3)
        {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let app_err = app.clone();
        s.resumes += 1;
        s.spawn_stream(move |e| {
            use tauri::Emitter;
            let _ = app_err.emit("profile-error", e);
        });
        // Re-present whatever was current (level loop or the same pair).
        s.shared
            .serial
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(profile_state_of(s, false))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Discard the running session entirely (its partial record included).
#[tauri::command]
fn profile_cancel() -> Result<(), String> {
    let mut guard = profile::PROFILE.lock().unwrap();
    if let Some(s) = guard.take() {
        s.stop_stream();
        let _ = std::fs::remove_file(profiles_dir().join(format!("{}.json", s.id)));
    }
    Ok(())
}

#[tauri::command]
fn profile_state() -> serde_json::Value {
    let guard = profile::PROFILE.lock().unwrap();
    match guard.as_ref() {
        Some(s) => {
            let paused = s.stream_done.load(std::sync::atomic::Ordering::Relaxed);
            serde_json::to_value(profile_state_of(s, paused)).unwrap()
        }
        None => serde_json::json!({ "active": false }),
    }
}

/// Saved profiles, newest first, journals stripped (they're large; the full
/// record stays on disk for provenance).
#[tauri::command]
fn profile_results() -> Vec<serde_json::Value> {
    let mut out: Vec<serde_json::Value> = std::fs::read_dir(profiles_dir())
        .map(|rd| {
            rd.filter_map(|e| {
                let text = std::fs::read_to_string(e.ok()?.path()).ok()?;
                let mut v: serde_json::Value = serde_json::from_str(&text).ok()?;
                if let Some(o) = v.as_object_mut() {
                    o.remove("journal");
                }
                Some(v)
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
fn profile_delete(id: String) -> Result<(), String> {
    if id.contains(['\\', '/', ':']) {
        return Err("invalid id".into());
    }
    std::fs::remove_file(profiles_dir().join(format!("{id}.json"))).map_err(|e| e.to_string())
}

/// Resume an unfinished session from its saved record after an app restart:
/// the staircases are rebuilt by replaying the journal (exact — proven by
/// the core replay test), the stream respawns, and the next trial presents.
#[tauri::command]
async fn profile_resume_saved(app: tauri::AppHandle, id: String) -> Result<ProfileState, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<ProfileState, String> {
        if profile_running() {
            return Err("a hearing-profile session is already running".into());
        }
        if TRIAL.lock().unwrap().is_some() || TRACK.lock().unwrap().is_some() {
            return Err(
                "stop other playback first — the profile needs the device exclusively".into(),
            );
        }
        if id.contains(['\\', '/', ':']) {
            return Err("invalid id".into());
        }
        let text = std::fs::read_to_string(profiles_dir().join(format!("{id}.json")))
            .map_err(|_| "saved session not found".to_string())?;
        let rec: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        if rec.get("finished").and_then(|v| v.as_bool()) == Some(true) {
            return Err("that session is already finished".into());
        }
        let rate =
            fletcher_core::playback::probe_rate(fletcher_core::playback::OutputMode::Exclusive)
                .map_err(|_| "the device refused exclusive mode".to_string())?;
        if rec.get("sampleRateHz").and_then(|v| v.as_u64()) != Some(rate as u64) {
            return Err("the device now runs at a different rate than the saved session".into());
        }
        let mut session = profile::ProfileSession::resume_from(&rec, rate)?;
        session
            .shared
            .serial
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let app_err = app.clone();
        session.spawn_stream(move |e| {
            use tauri::Emitter;
            let _ = app_err.emit("profile-error", e);
        });
        if session.anchor_dbfs.is_some() {
            session.next_trial();
        }
        let state = profile_state_of(&session, false);
        *profile::PROFILE.lock().unwrap() = Some(session);
        Ok(state)
    })
    .await
    .map_err(|e| e.to_string())?
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
    if profile_running() {
        return Err("a hearing-profile session is running — finish or cancel it first".into());
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
        "DELETE FROM battery_clips WHERE kind = 'track' AND clip_id = ?1",
        rusqlite::params![id],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "DELETE FROM battery_clips WHERE kind = 'clip' AND clip_id IN (SELECT id FROM clips WHERE track_id = ?1)",
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
        let preamp_a = engine_preamp_full(&specs).0;
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
    if profile_running() {
        return Err("a hearing-profile session is running — finish or cancel it first".into());
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

        // ADR-0016 (owner's choice for review F8): every song is first brought
        // to one loudness, identically in BOTH play methods, so Flat ↔ EQ is
        // matched and song ↔ song stays fixed. Attenuate-only (TB-06).
        let song_db = lufs_flat
            .map(|l| (SONG_NORM_LUFS - l).min(0.0))
            .unwrap_or(0.0);
        // Then the listening level. On the SHARED path (EQ mode, or Flat when the
        // device refused exclusive), APO and Windows already apply Fletcher's
        // preamp, outside Preamp lines and the volume, so nothing more is added.
        // The EXCLUSIVE path skips all of that, so the engine recreates it. This
        // branches on the actual output mode, not on the method asked for
        // (review F2: shared EQ used to get it twice, about 10 dB too quiet).
        let gain_db = if exclusive {
            song_db + exclusive_listening_db(compared_flat_preamp_db())
        } else {
            song_db
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
    preamp_trim_db: f64,
    /// Attenuation reachable from config.txt that Fletcher does NOT own
    /// (ADR-0007) — hand-added `Preamp:` lines, other tools' files. The app
    /// cannot edit it, so it must at least NAME it: this is the number that
    /// makes "why is it quiet and why can't I fix it" answerable (TB-31).
    foreign_preamp_db: f64,
    trim_min_db: f64,
    trim_max_db: f64,
    level_matching: bool,
    apo_install_path: Option<String>,
    apo_config_path: Option<String>,
}

/// Sum of every `Preamp:` reachable from config.txt that isn't ours. Fletcher
/// may not edit these (ADR-0007) but it must be able to explain them.
fn foreign_preamp_db() -> f64 {
    let o = outside_path();
    o.cut_db + o.boost_db
}

#[tauri::command]
fn settings_state() -> SettingsState {
    let apo = apo::detect().ok();
    SettingsState {
        reference_db: reference_db(),
        preamp_trim_db: preamp_trim_db(),
        foreign_preamp_db: foreign_preamp_db(),
        trim_min_db: TRIM_MIN_DB,
        trim_max_db: TRIM_MAX_DB,
        level_matching: level_matching(),
        apo_install_path: apo.as_ref().map(|i| i.install_path.display().to_string()),
        apo_config_path: apo.as_ref().map(|i| i.config_path.display().to_string()),
    }
}

/// Set the output trim. Mirrors `set_reference_db`'s guards: changing absolute
/// loudness mid-session would desynchronize what's playing from what the trial
/// believes is playing (TB-26), and mid-profile it invalidates the level the
/// whole measurement is pinned to (TB-29).
#[tauri::command]
fn set_preamp_trim(db: f64) -> Result<EqState, String> {
    if TRIAL.lock().unwrap().is_some() {
        return Err("a blind test is running — finish or cancel it first".into());
    }
    if profile_running() {
        return Err("a hearing-profile session is running — finish or cancel it first".into());
    }
    write_state_field(
        "preampTrimDb",
        serde_json::json!(db.clamp(TRIM_MIN_DB, TRIM_MAX_DB)),
    );
    apply_side(&ab_side())?;
    eq_state()
}

/// Set the global reference loudness (Q-06/Q-16 placeholder — the noise
/// calibration flow replaces the raw number once the signal generator exists).
/// Re-applies the current side so the change is immediately audible.
#[tauri::command]
fn set_reference_db(db: f64) -> Result<EqState, String> {
    if TRIAL.lock().unwrap().is_some() {
        return Err("a blind test is running — finish or cancel it first".into());
    }
    if profile_running() {
        return Err("a hearing-profile session is running — finish or cancel it first".into());
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
    if profile_running() {
        return Err("a hearing-profile session is running — finish or cancel it first".into());
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
    /// `apo::DeviceApo` for this device — lets the picker steer toward a
    /// device where the EQ is actually audible.
    apo: &'static str,
}

#[tauri::command]
fn devices_list() -> Vec<DeviceDto> {
    devices::list_render_devices()
        .into_iter()
        .map(|d| DeviceDto {
            apo: device_apo(&d.id).as_str(),
            id: d.id,
            name: d.name,
            is_default: d.is_default,
        })
        .collect()
}

/// Devices this process has seen WITHOUT APO. A device that was Missing and is
/// now Active was just registered: APO 1.4.2 went live at once (observed
/// 2026-10-26), older versions may need a reboot, so the UI says
/// both (TB-32). Process-lifetime only: after an app restart the evidence is gone
/// and we just say "active".
static APO_SEEN_MISSING: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

fn device_apo(id: &str) -> apo::DeviceApo {
    let s = apo::device_apo_status(id);
    if s == apo::DeviceApo::Missing {
        let mut seen = APO_SEEN_MISSING.lock().unwrap();
        if !seen.iter().any(|x| x == id) {
            seen.push(id.to_string());
        }
    }
    s
}

fn default_device_apo() -> (&'static str, bool) {
    let Some(id) = devices::default_render_device_id() else {
        return (apo::DeviceApo::Unknown.as_str(), false);
    };
    let s = device_apo(&id);
    let pending = s == apo::DeviceApo::Active && APO_SEEN_MISSING.lock().unwrap().contains(&id);
    (s.as_str(), pending)
}

/// Open APO's own device selector (elevated) so the user can tick the device.
/// Returns immediately; when the selector closes we re-push state so the
/// banner updates without the user doing anything (TB-32).
#[tauri::command]
fn apo_open_device_selector(app: tauri::AppHandle) -> Result<(), String> {
    let install = apo::detect().map_err(|e| e.to_string())?;
    apo::device_selector_path(&install).ok_or_else(|| {
        format!(
            "couldn't find APO's device selector in {}",
            install.install_path.display()
        )
    })?;
    std::thread::spawn(move || {
        use tauri::Emitter;
        if let Err(e) = apo::run_device_selector(&install) {
            let _ = app.emit("apo-selector-error", e);
        }
        let _ = app.emit("apo-config-changed", ());
    });
    Ok(())
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
    /// The loudness both sides play at, on the reference scale: reference +
    /// trim − shortfall. What the user actually hears, never a raw preamp (TB-31).
    level_db: f64,
    /// TB-08: dB by which BOTH sides sit below reference + trim because the
    /// active chain hit its clip ceiling. They still match each other
    /// (`matched_pair`); 0 = both exactly on the target.
    shortfall_db: f64,
    /// False when the two sides CAN'T be matched (an infeasible chain, F5): the
    /// bar must then say unmatched, never "matched".
    matched: bool,
    /// The last flip was measured on the Clip Studio song, for this chain.
    measured: bool,
}

#[tauri::command]
fn ab_info() -> AbInfo {
    let chain = active_chain();
    let (_, _, shortfall_db, matched) = matched_pair(&specs_of(&chain), &[]);
    AbInfo {
        side: ab_side(),
        level_db: round_tenth(reference_db().min(0.0) + preamp_trim_db() - shortfall_db),
        shortfall_db,
        matched,
        measured: studio_measured_now(&chain),
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
/// `clip_id: None` = a whole song (the full track is the material).
#[derive(Clone)]
struct TrialClip {
    clip_id: Option<i64>,
    track_id: i64,
    clip_name: String,
    track_title: String,
    frame_in: u64,
    frame_out: u64,
    /// True-LUFS trim bringing bus B to bus A's loudness on this clip.
    trim_b_db: f64,
    /// Bus A's share (≤ 0): when B couldn't come up far enough without
    /// clipping, A came down by the rest, so the two are equal (review F1).
    trim_a_db: f64,
    /// Kept for record compatibility; always 0 now that A takes the remainder.
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
            let (pa, pb, _, _) = matched_pair(&specs_of(&session.a), &specs_of(&session.b));
            if hear_a {
                activate_chain_at(&session.a, pa)?;
            } else {
                activate_chain_at(&session.b, pb)?;
            }
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
    let (clip_name, track_title, clip_index, clip_count, battery_name, suspended) =
        match &session.material {
            Material::System => (None, None, None, None, None, false),
            Material::Clips {
                plan,
                battery_name,
                suspended,
                ..
            } => {
                let cur = plan.get(trial.min(plan.len().saturating_sub(1)));
                let distinct: std::collections::HashSet<i64> = plan.iter().map(plan_key).collect();
                (
                    cur.map(|c| c.clip_name.clone()),
                    cur.map(|c| c.track_title.clone()),
                    cur.map(|c| {
                        // Position within the distinct battery, 1-based.
                        let mut seen = std::collections::HashSet::new();
                        let mut idx = 0;
                        for p in plan.iter().take(trial + 1) {
                            if seen.insert(plan_key(p)) {
                                idx += 1;
                            }
                            if plan_key(p) == plan_key(c) {
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
        track_ids: Option<Vec<i64>>,
    },
}

/// Stable identity of a plan entry: clip id, or −track_id for whole songs.
fn plan_key(c: &TrialClip) -> i64 {
    c.clip_id.unwrap_or(-c.track_id)
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
    let (pa, pb, _, _) = engine_pair(&sa, &sb);
    {
        let mut bus = session.shared.bus.lock().unwrap();
        *bus = engine::BusConfig {
            specs: sa,
            preamp_a_db: pa,
            specs_b: sb,
            preamp_b_db: pb,
            trim_b_db: clip.trim_b_db,
            trim_a_db: clip.trim_a_db,
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
        let freqs = log_freqs(CURVE_FMIN, 20000.0, CURVE_POINTS);
        let (sa, sb) = (specs_of(&chain_a), specs_of(&chain_b));
        let (pa, pb, _, _) = matched_pair(&sa, &sb);
        let ra = chain_response_db(&sa, pa, FS, &freqs);
        let rb = chain_response_db(&sb, pb, FS, &freqs);
        if ra.iter().zip(&rb).all(|(x, y)| (x - y).abs() < 0.05) {
            return Err(
                "A and B level-match to the same response — there is no difference to test".into(),
            );
        }
    }
    if TRIAL.lock().unwrap().is_some() {
        return Err("a blind test is already running — finish or cancel it first".into());
    }
    if profile_running() {
        return Err("a hearing-profile session is running — finish or cancel it first".into());
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
            track_ids,
        }) => trial_build_clips_material(
            &chain_a, &chain_b, battery_id, clip_ids, track_ids, cap, &mut rng,
        )?,
    };

    // "Matched" only if it truly can be: the setting is on AND the pair is
    // feasible (an infeasible chain can't be matched, review F5). Clip trials
    // additionally measure and correct per clip (`dsp::measured_trims`).
    let (sa, sb) = (specs_of(&chain_a), specs_of(&chain_b));
    let pair_ok = match material {
        Material::System => matched_pair(&sa, &sb).3,
        Material::Clips { .. } => engine_pair(&sa, &sb).3,
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
        level_matched: level_matching() && pair_ok,
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
    track_ids: Option<Vec<i64>>,
    cap: usize,
    rng: &mut fletcher_core::stats::Xorshift,
) -> Result<Material, String> {
    let conn = library_db()?;
    let (items, battery_name) = match (battery_id, clip_ids, track_ids) {
        (Some(bid), _, _) => {
            let name: String = conn
                .query_row(
                    "SELECT name FROM batteries WHERE id = ?1",
                    rusqlite::params![bid],
                    |r| r.get(0),
                )
                .map_err(|_| "battery not found".to_string())?;
            let mut stmt = conn
                .prepare(
                    "SELECT clip_id, kind FROM battery_clips WHERE battery_id = ?1 ORDER BY ord",
                )
                .map_err(|e| e.to_string())?;
            let items: Vec<BatteryItem> = stmt
                .query_map(rusqlite::params![bid], |r| {
                    Ok(BatteryItem {
                        id: r.get(0)?,
                        kind: r.get(1)?,
                    })
                })
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            (items, Some(name))
        }
        (None, cids, tids) => {
            let mut items: Vec<BatteryItem> = Vec::new();
            for id in cids.unwrap_or_default() {
                items.push(BatteryItem {
                    kind: "clip".into(),
                    id,
                });
            }
            for id in tids.unwrap_or_default() {
                items.push(BatteryItem {
                    kind: "track".into(),
                    id,
                });
            }
            if items.is_empty() {
                return Err("pick a battery, clips, or songs to test on".into());
            }
            (items, None)
        }
    };
    if items.is_empty() {
        return Err("that battery is empty — add clips or songs to it first".into());
    }
    // Fetch rows (names denormalized for provenance). Whole songs span
    // 0 → end (t_out clamps to the real length at measurement).
    struct ClipRef {
        clip_id: Option<i64>,
        track_id: i64,
        name: String,
        title: String,
        t_in: f64,
        t_out: f64,
    }
    let mut clips: Vec<ClipRef> = Vec::new();
    for item in &items {
        if item.kind == "track" {
            let (title, dur): (String, Option<f64>) = conn
                .query_row(
                    "SELECT title, duration_s FROM tracks WHERE id = ?1",
                    rusqlite::params![item.id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .map_err(|_| format!("track #{} no longer exists", item.id))?;
            clips.push(ClipRef {
                clip_id: None,
                track_id: item.id,
                name: title.clone(),
                title,
                t_in: 0.0,
                t_out: dur.unwrap_or(f64::MAX),
            });
            continue;
        }
        let cid = item.id;
        let row = conn
            .query_row(
                "SELECT c.id, c.track_id, c.name, t.title, c.t_in, c.t_out
                 FROM clips c JOIN tracks t ON t.id = c.track_id WHERE c.id = ?1",
                rusqlite::params![cid],
                |r| {
                    Ok(ClipRef {
                        clip_id: Some(r.get(0)?),
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
    // region, then the measured correction (`dsp::measured_trims`): B takes it,
    // and if B can't come up far enough without clipping, A comes down by the
    // rest, so no residual is ever labelled matched (review F1). Silent clips
    // are refused (F9). Per-track stream gain from the loudest clip, so nothing
    // exceeds the target and clips keep their natural relative loudness.
    let (sa, sb) = (specs_of(chain_a), specs_of(chain_b));
    let (pa, pb, _, _) = engine_pair(&sa, &sb);
    // The same level Clip Studio plays at: songs at SONG_NORM_LUFS, then the
    // listening level the shared path would apply (exclusive output skips it).
    let target = SONG_NORM_LUFS
        + exclusive_listening_db(level_plan(&[], preamp_trim_db(), &outside_path()).0);
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
            let fo = if c.t_out == f64::MAX {
                total
            } else {
                ((c.t_out * rate as f64) as u64).clamp(fi + 1, total)
            };
            if fo.saturating_sub(fi) < (rate as u64) / 2 {
                return Err(format!(
                    "\"{}\" is under 0.5 s of audio — too short for a blind switch",
                    c.name
                ));
            }
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
            let (trim_a, trim_b) = dsp::measured_trims(lufs_a, lufs_b, peak_b, 0.3)
                .map_err(|why| format!("\"{}\" {why} — pick a clip with sound in it", c.name))?;
            let g = (target - (lufs_a + trim_a)).min(0.0);
            let e = track_gain.entry(*tid).or_insert(g);
            *e = e.min(g);
            measured.insert(
                c.clip_id.unwrap_or(-c.track_id),
                TrialClip {
                    clip_id: c.clip_id,
                    track_id: c.track_id,
                    clip_name: c.name.clone(),
                    track_title: c.title.clone(),
                    frame_in: fi,
                    frame_out: fo,
                    trim_b_db: trim_b,
                    trim_a_db: trim_a,
                    trim_shortfall_db: 0.0,
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
                .map(|c| c.clip_id.unwrap_or(-c.track_id))
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
    /// How the two sides were levelled (ADR-0016), so a result can be judged
    /// later: both sides sat this many dB below the target because one of them
    /// hit its clip ceiling (0 = both on target)…
    pair_below_db: f64,
    /// …using this loudness method: "k-weighted-pink" (system audio, estimated
    /// from the EQ curves) or "measured-lufs" (clips, measured on the audio).
    level_method: &'static str,
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
            let mut song_track_ids = Vec::new();
            let mut trims = Vec::new();
            for c in plan {
                if seen.insert(plan_key(c)) {
                    match c.clip_id {
                        Some(id) => clip_ids.push(id),
                        None => song_track_ids.push(c.track_id),
                    }
                    trims.push(serde_json::json!({
                        "clipId": c.clip_id,
                        "trackId": c.track_id,
                        "trimBDb": c.trim_b_db,
                        "trimADb": c.trim_a_db,
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
                    "trackIds": song_track_ids,
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
                clip_id: clip.and_then(|c| c.clip_id),
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
        pair_below_db: match session.material {
            Material::System => matched_pair(&specs_of(&session.a), &specs_of(&session.b)).2,
            Material::Clips { .. } => engine_pair(&specs_of(&session.a), &specs_of(&session.b)).2,
        },
        level_method: match session.material {
            Material::System => "k-weighted-pink",
            Material::Clips { .. } => "measured-lufs",
        },
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
                    bus.trim_a_db = next.trim_a_db;
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
        // Recursive: includes can live in subfolders, and they count (R2/R3).
        if watcher
            .watch(&install.config_path, RecursiveMode::Recursive)
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
        // Fletcher's plan depends on the WHOLE outside path (ADR-0016): Preamp
        // lines, outside filters, their scopes, nested includes, unreadable
        // files. When any of it changes, re-plan and rewrite at once (follow-up
        // R3: it used to watch only the old preamp total, so turning an outside
        // cut off, or changing a scope, left a stale preamp in the file). Our
        // own rewrite never changes the outside path, so this can't loop.
        let fingerprint = || format!("{:?}", outside_path());
        let mut known = fingerprint();
        while let Ok(ev) = rx.recv() {
            let Ok(ev) = ev else { continue };
            if !relevant(&ev) {
                continue;
            }
            // Debounce bursts (editors and Peace write several events per save).
            std::thread::sleep(std::time::Duration::from_millis(150));
            while rx.try_recv().is_ok() {}
            let now = fingerprint();
            if now != known {
                known = now;
                replan_after_outside_change();
            }
            let _ = handle.emit("apo-config-changed", ());
        }
    });
}

/// The outside path changed: rewrite what's playing at its new plan. A running
/// SYSTEM-audio trial gets its current audition re-applied (both sides are
/// planned against the same path, so the pair stays matched); clip trials and
/// the hearing profile play exclusively, untouched by APO, so they're left alone.
fn replan_after_outside_change() {
    if profile_running() {
        return;
    }
    let mut guard = TRIAL.lock().unwrap();
    if let Some(session) = guard.as_mut() {
        if matches!(session.material, Material::System) {
            let audition = session.audition.clone();
            let _ = trial_apply_audition(session, &audition);
        }
        return;
    }
    drop(guard);
    let _ = apply_side(&ab_side());
}

/// Bring the main window back from the tray, unminimized and focused. Used by
/// the tray's "Show Fletcher" and by a second launch (single instance).
fn show_main_window(app: &tauri::AppHandle) {
    use tauri::Manager;
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
}

fn setup_tray(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::TrayIconBuilder;

    let show = MenuItem::with_id(app, "show", "Show Fletcher", true, None::<&str>)?;
    let flip = MenuItem::with_id(app, "flip", tray_flip_text(), true, None::<&str>)?;
    *TRAY_FLIP.lock().unwrap() = Some(flip.clone());
    let quit = MenuItem::with_id(app, "quit", "Quit Fletcher", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &flip, &quit])?;

    let mut tray = TrayIconBuilder::new()
        .menu(&menu)
        .tooltip("Fletcher — honest EQ")
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main_window(app),
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

/// The pop-outs whose Ctrl+Z / Ctrl+Y walk the main window's undo rail.
fn has_undo_keys(label: &str) -> bool {
    matches!(label, "history" | "filter")
}

/// Which pop-out currently owns the undo shortcuts. Moving straight from one
/// such window to another can deliver "B focused" before "A blurred"; only the
/// owner may unregister, so A's blur can't take B's keys away.
static UNDO_KEYS_OWNER: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

fn undo_keys_focus(app: &tauri::AppHandle, label: &str, focused: bool) {
    let mut owner = UNDO_KEYS_OWNER.lock().unwrap();
    if focused {
        if owner.is_none() {
            set_history_shortcuts(app, true);
        }
        *owner = Some(label.to_string());
    } else if owner.as_deref() == Some(label) {
        set_history_shortcuts(app, false);
        *owner = None;
    }
}

/// Undo/redo shortcuts for the undo-capable pop-outs, active only while one has
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

// ---------------- Global hotkeys (remappable; GitHub issue #5) ----------------
//
// Every global hotkey is an ACTION with a default combination. state.json's
// "hotkeys" object maps action id → combination (missing = the default), and
// "hotkeysOff" lists the actions switched off. Off keeps the combination, so
// switching back on restores it. Combinations use the browser's KeyboardEvent.code
// names (ctrl+shift+KeyA), which the shortcut plugin parses directly, so the
// remapping UI can store exactly what it captured. Add a row to HOTKEYS (and
// a case in `run_hotkey`) for each new action.

struct HotkeyAction {
    id: &'static str,
    label: &'static str,
    default: &'static str,
}

const HOTKEYS: &[HotkeyAction] = &[HotkeyAction {
    id: "ab_flip",
    label: "Flip A/B",
    default: "ctrl+shift+KeyA",
}];

/// System-wide editing shortcuts a global hotkey would steal from every app.
const RESERVED_COMBOS: &[&str] = &[
    "ctrl+KeyZ",
    "ctrl+KeyY",
    "ctrl+shift+KeyZ",
    "ctrl+KeyC",
    "ctrl+KeyV",
    "ctrl+KeyX",
    "ctrl+KeyA",
    "ctrl+KeyS",
];

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HotkeyInfo {
    id: &'static str,
    label: &'static str,
    /// The chosen combination (kept while switched off).
    combo: String,
    enabled: bool,
    default: &'static str,
}

/// The chosen combination for an action: stored, else the default.
fn hotkey_choice(id: &str) -> String {
    let default = HOTKEYS
        .iter()
        .find(|a| a.id == id)
        .map_or("", |a| a.default);
    read_state()
        .get("hotkeys")
        .and_then(|h| h.get(id))
        .and_then(|v| v.as_str())
        .unwrap_or(default)
        .to_string()
}

fn hotkey_enabled(id: &str) -> bool {
    let state = read_state();
    // A stored null came from an earlier build's "off" (before the switch).
    let legacy_off = state
        .get("hotkeys")
        .and_then(|h| h.get(id))
        .is_some_and(|v| v.is_null());
    let off = state
        .get("hotkeysOff")
        .and_then(|v| v.as_array())
        .is_some_and(|ids| ids.iter().any(|v| v.as_str() == Some(id)));
    !(off || legacy_off)
}

/// The combination actually registered: the choice, if switched on.
fn hotkey_combo(id: &str) -> Option<String> {
    hotkey_enabled(id).then(|| hotkey_choice(id))
}

fn hotkey_infos() -> Vec<HotkeyInfo> {
    HOTKEYS
        .iter()
        .map(|a| HotkeyInfo {
            id: a.id,
            label: a.label,
            combo: hotkey_choice(a.id),
            enabled: hotkey_enabled(a.id),
            default: a.default,
        })
        .collect()
}

fn run_hotkey(app: &tauri::AppHandle, id: &str) {
    if id == "ab_flip" {
        ab_flip(app)
    }
}

/// Parse a combination, normalized for comparison. Errors are user-facing.
fn parse_combo(combo: &str) -> Result<tauri_plugin_global_shortcut::Shortcut, String> {
    combo
        .parse::<tauri_plugin_global_shortcut::Shortcut>()
        .map_err(|_| {
            format!(
                "{} isn't a key combination Windows can register",
                pretty_combo(combo)
            )
        })
}

/// "ctrl+shift+KeyA" → "Ctrl+Shift+A", for the tray menu and messages.
fn pretty_combo(combo: &str) -> String {
    combo
        .split('+')
        .map(|part| match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => "Ctrl".to_string(),
            "shift" => "Shift".to_string(),
            "alt" => "Alt".to_string(),
            "super" | "meta" | "cmd" => "Win".to_string(),
            _ => part
                .strip_prefix("Key")
                .or_else(|| part.strip_prefix("Digit"))
                .unwrap_or(part)
                .to_string(),
        })
        .collect::<Vec<_>>()
        .join("+")
}

/// Register one action's combination. Fails when another app holds it.
fn register_hotkey(app: &tauri::AppHandle, id: &'static str, combo: &str) -> Result<(), String> {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
    let shortcut = parse_combo(combo)?;
    app.global_shortcut()
        .on_shortcut(shortcut, move |app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                run_hotkey(app, id);
            }
        })
        .map_err(|_| format!("{} is already used by another app", pretty_combo(combo)))
}

fn unregister_hotkey(app: &tauri::AppHandle, combo: &str) {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    if let Ok(shortcut) = parse_combo(combo) {
        let _ = app.global_shortcut().unregister(shortcut);
    }
}

/// The tray's "Flip A/B" item, kept so its shortcut label can follow a remap.
static TRAY_FLIP: std::sync::Mutex<Option<tauri::menu::MenuItem<tauri::Wry>>> =
    std::sync::Mutex::new(None);

fn tray_flip_text() -> String {
    match hotkey_combo("ab_flip") {
        Some(c) => format!("Flip A/B\t{}", pretty_combo(&c)),
        None => "Flip A/B".to_string(),
    }
}

/// While the remapping UI listens for keys, the current hotkeys are released,
/// so pressing the old combination to try it doesn't fire it.
static HOTKEYS_PAUSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn register_all_hotkeys(app: &tauri::AppHandle) {
    for a in HOTKEYS {
        if let Some(combo) = hotkey_combo(a.id) {
            if let Err(e) = register_hotkey(app, a.id, &combo) {
                eprintln!("fletcher: hotkey {}: {e}", a.id);
            }
        }
    }
}

fn setup_hotkey(app: &tauri::App) {
    register_all_hotkeys(app.handle());
}

#[tauri::command]
fn hotkeys() -> Vec<HotkeyInfo> {
    hotkey_infos()
}

/// Remap one action and/or switch it on or off. Validated, registered live,
/// saved; on any failure the previous setup stays in force. The combination is
/// remembered while off.
#[tauri::command]
fn hotkey_set(
    app: tauri::AppHandle,
    id: String,
    combo: Option<String>,
    enabled: Option<bool>,
) -> Result<Vec<HotkeyInfo>, String> {
    let action = HOTKEYS
        .iter()
        .find(|a| a.id == id)
        .ok_or_else(|| format!("unknown hotkey {id}"))?;
    let old = hotkey_combo(action.id);
    let picked = combo.is_some();
    let choice = combo.unwrap_or_else(|| hotkey_choice(action.id));
    let on = enabled.unwrap_or_else(|| hotkey_enabled(action.id));
    // Check a newly picked combination even while switched off, so an invalid
    // one is refused when chosen, not later when switching back on.
    let check = (on || picked).then(|| choice.clone());
    let combo = on.then(|| choice.clone());
    if let Some(c) = &check {
        let wanted = parse_combo(c)?;
        let mods: Vec<String> = c.split('+').map(|p| p.to_ascii_lowercase()).collect();
        if !mods
            .iter()
            .any(|m| matches!(m.as_str(), "ctrl" | "control" | "alt" | "super" | "meta"))
        {
            return Err(
                "Use Ctrl, Alt or Win in the combination, so it can't take over normal typing"
                    .into(),
            );
        }
        if RESERVED_COMBOS
            .iter()
            .any(|r| parse_combo(r).map(|s| s == wanted).unwrap_or(false))
        {
            return Err(format!(
                "{} is a standard editing shortcut in every app; pick another",
                pretty_combo(c)
            ));
        }
        for other in HOTKEYS.iter().filter(|a| a.id != action.id) {
            if hotkey_combo(other.id)
                .is_some_and(|o| parse_combo(&o).map(|s| s == wanted).unwrap_or(false))
            {
                return Err(format!("{} is already {}", pretty_combo(c), other.label));
            }
        }
    }
    let live = !HOTKEYS_PAUSED.load(std::sync::atomic::Ordering::Relaxed);
    if live {
        if let Some(o) = &old {
            unregister_hotkey(&app, o);
        }
        if let Some(c) = &combo {
            if let Err(e) = register_hotkey(&app, action.id, c) {
                if let Some(o) = &old {
                    let _ = register_hotkey(&app, action.id, o);
                }
                return Err(e);
            }
        }
    }
    let state = read_state();
    let mut map = state
        .get("hotkeys")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    map[action.id] = serde_json::Value::from(choice);
    write_state_field("hotkeys", map);
    let mut off: Vec<String> = state
        .get("hotkeysOff")
        .and_then(|v| v.as_array())
        .map(|ids| {
            ids.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    off.retain(|x| x != action.id);
    if !on {
        off.push(action.id.to_string());
    }
    write_state_field("hotkeysOff", serde_json::json!(off));
    if action.id == "ab_flip" {
        if let Some(item) = TRAY_FLIP.lock().unwrap().as_ref() {
            let _ = item.set_text(tray_flip_text());
        }
    }
    Ok(hotkey_infos())
}

/// Release (true) or restore (false) every hotkey while the remapping UI
/// captures keys. Restoring re-registers whatever is configured by then.
#[tauri::command]
fn hotkeys_pause(app: tauri::AppHandle, paused: bool) {
    use std::sync::atomic::Ordering::Relaxed;
    if HOTKEYS_PAUSED.swap(paused, Relaxed) == paused {
        return;
    }
    if paused {
        for a in HOTKEYS {
            if let Some(c) = hotkey_combo(a.id) {
                unregister_hotkey(&app, &c);
            }
        }
    } else {
        register_all_hotkeys(&app);
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Must be the first plugin. A second launch (Start menu, autostart, a
        // double-click while it's in the tray) brings this window forward and
        // exits, instead of starting a second copy that fights over fletcher.txt
        // and the A/B hotkey.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app)
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
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
                    } else if has_undo_keys(window.label()) {
                        undo_keys_focus(window.app_handle(), window.label(), false);
                    }
                }
                // The pop-out webview doesn't receive DOM keyboard events, so
                // its undo/redo keys are OS-level shortcuts registered only
                // while it is focused — they never leak into other apps.
                tauri::WindowEvent::Focused(focused) if has_undo_keys(window.label()) => {
                    undo_keys_focus(window.app_handle(), window.label(), *focused);
                }
                tauri::WindowEvent::Destroyed if window.label() == "filter" => {
                    undo_keys_focus(window.app_handle(), "filter", false);
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
                    undo_keys_focus(window.app_handle(), "history", false);
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
            apo_open_device_selector,
            updates::update_check,
            updates::update_install,
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
            profile_start,
            profile_set_level,
            profile_accept_level,
            profile_answer,
            profile_replay,
            profile_pause,
            profile_resume,
            profile_resume_saved,
            profile_cancel,
            profile_state,
            profile_results,
            profile_delete,
            autoeq_search,
            autoeq_import,
            rew_import,
            preset_rename,
            parse_filters,
            chain_curves,
            filter_responses,
            hotkeys,
            hotkey_set,
            hotkeys_pause,
            preview_chain,
            preset_create_from_chain,
            settings_state,
            set_reference_db,
            set_preamp_trim,
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

#[cfg(test)]
mod outside_path_tests {
    //! The plain-path rule (ADR-0016) on real files in a temp folder.
    use super::*;

    fn read(name: &str, files: &[(&str, &str)]) -> OutsidePath {
        let dir =
            std::env::temp_dir().join(format!("fletcher-outside-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (file, text) in files {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(file), text).unwrap();
        }
        let out = outside_path_in(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        out
    }

    #[test]
    fn a_plain_cut_is_headroom() {
        // The owner's own setup: an empty peace.txt, Fletcher, and −10 dB.
        let out = read(
            "plain",
            &[
                (
                    "config.txt",
                    "# note\nInclude: peace.txt\nInclude: fletcher.txt\n\nPreamp: -10 dB\n",
                ),
                ("peace.txt", ""),
            ],
        );
        assert!(out.plain);
        assert_eq!(
            (out.ceiling_preamp_db(), out.exclusive_preamp_db()),
            (-10.0, -10.0)
        );
    }

    #[test]
    fn anything_else_on_the_path_earns_no_headroom() {
        for (name, extra) in [
            ("filter", "Filter: ON PK Fc 100 Hz Gain 9 dB Q 1\n"),
            ("graphiceq", "GraphicEQ: 25 6; 100 0\n"),
            ("scope", "Device: Speakers\n"),
            ("nested", "Include: more.txt\n"),
        ] {
            let out = read(
                name,
                &[
                    ("config.txt", "Preamp: -6 dB\nInclude: other.txt\n"),
                    ("other.txt", extra),
                ],
            );
            assert!(!out.plain, "{name}");
            assert_eq!(
                out.ceiling_preamp_db(),
                0.0,
                "{name}: a cut it can't prove isn't headroom"
            );
            assert_eq!(
                out.exclusive_preamp_db(),
                -6.0,
                "{name}: exclusive still takes the cut"
            );
        }
    }

    #[test]
    fn boosts_always_count_and_missing_files_err_quiet() {
        let boost = read("boost", &[("config.txt", "Device: X\nPreamp: 4 dB\n")]);
        assert_eq!(boost.ceiling_preamp_db(), 4.0);
        assert_eq!(
            boost.exclusive_preamp_db(),
            0.0,
            "never louder in exclusive"
        );
        let missing = read(
            "missing",
            &[("config.txt", "Preamp: -6 dB\nInclude: gone.txt\n")],
        );
        assert!(missing.unreadable && !missing.plain);
        assert_eq!(missing.ceiling_preamp_db(), 0.0);
        assert_eq!(missing.exclusive_preamp_db(), -16.0);
    }
}

#[cfg(test)]
mod level_write_tests {
    use super::*;

    #[test]
    fn fades_never_rise_more_than_3_db_per_write() {
        // Review 2, P1-1: a reference −30 → 0 used to jump 19 dB in one write.
        for rise in [3.5, 6.0, 12.0, 19.0, 30.0, 36.0, 80.0, f64::INFINITY] {
            let steps = fade_steps(rise);
            assert!(steps.len() <= 12, "rise {rise}: {} writes", steps.len());
            let start = rise.min(36.0);
            let mut prev = 0.0 - start; // relative to the final level
            for below in steps.iter().chain(&[0.0]) {
                assert!(
                    -below - prev <= MAX_JUMP_DB + 1e-9,
                    "rise {rise}: {prev} → {}",
                    -below
                );
                prev = -below;
            }
        }
        assert!(fade_steps(3.0).is_empty() && fade_steps(f64::NAN).is_empty());
    }

    #[test]
    fn the_louder_side_comes_down_and_nothing_rises() {
        // diff = LUFS(A) − LUFS(B) on the song.
        assert_eq!(studio_side_trim_db(4.0, "a"), -4.0);
        assert_eq!(studio_side_trim_db(4.0, "b"), 0.0);
        assert_eq!(studio_side_trim_db(-6.0, "b"), -6.0);
        assert_eq!(studio_side_trim_db(-6.0, "a"), 0.0);
    }
}
