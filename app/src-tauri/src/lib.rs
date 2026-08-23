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

fn active_preset() -> Option<String> {
    let text = std::fs::read_to_string(data_dir().join("state.json")).ok()?;
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()?
        .get("activePreset")?
        .as_str()
        .map(String::from)
}

fn set_active_preset(name: Option<&str>) {
    let _ = std::fs::create_dir_all(data_dir());
    let json = serde_json::json!({ "activePreset": name });
    let _ = fsx::write_atomic(&data_dir().join("state.json"), &json.to_string());
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

/// Replace Fletcher's own chain (fletcher.txt) with the given filters.
/// Auto-preamp is computed from the summed response peak (TB-06), the file
/// is written atomically (TB-11), and APO hot-reloads it. Returns the fresh
/// merged state.
#[tauri::command]
fn set_fletcher_chain(filters: Vec<FilterIn>) -> Result<EqState, String> {
    let install = apo::detect().map_err(|e| e.to_string())?;

    let chain: Vec<ChainFilter> = filters
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
        .collect::<Result<_, String>>()?;

    let specs: Vec<FilterSpec> = chain
        .iter()
        .filter(|f| f.enabled)
        .map(|f| FilterSpec {
            kind: f.kind,
            fc_hz: f.fc_hz,
            gain_db: f.gain_db,
            q: f.q,
        })
        .collect();
    let preamp = auto_preamp_db(&specs, FS);

    let text = render_fletcher_file(preamp, &chain);
    fsx::write_atomic(&install.config_path.join("fletcher.txt"), &text)
        .map_err(|e| e.to_string())?;

    // Edits persist into the active preset, so switching away and back keeps them.
    if let Some(name) = active_preset() {
        let _ = store()?.save(&name, preamp, &chain);
    }

    eq_state()
}

/// Rewrite fletcher.txt from a chain (with fresh auto-preamp).
fn activate_chain(chain: &[ChainFilter]) -> Result<(), String> {
    let install = apo::detect().map_err(|e| e.to_string())?;
    let specs: Vec<FilterSpec> = chain
        .iter()
        .filter(|f| f.enabled)
        .map(|f| FilterSpec {
            kind: f.kind,
            fc_hz: f.fc_hz,
            gain_db: f.gain_db,
            q: f.q,
        })
        .collect();
    let preamp = auto_preamp_db(&specs, FS);
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
    let specs: Vec<FilterSpec> = chain
        .iter()
        .filter(|f| f.enabled)
        .map(|f| FilterSpec {
            kind: f.kind,
            fc_hz: f.fc_hz,
            gain_db: f.gain_db,
            q: f.q,
        })
        .collect();
    st.save(&name, auto_preamp_db(&specs, FS), &chain)
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            spawn_config_watcher(app.handle().clone());
            Ok(())
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
            preset_delete
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
