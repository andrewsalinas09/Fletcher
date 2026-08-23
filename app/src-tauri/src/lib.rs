//! Fletcher's Tauri shell: thin command layer over fletcher-core (ADR-0001/0005 —
//! the UI draws, the core thinks). DTOs live here so the core stays serde-free.

use fletcher_core::config::{ConfigDoc, Parsed};
use fletcher_core::dsp::{chain_response_db, log_freqs, FilterSpec};
use fletcher_core::{apo, devices, dsp};
use serde::Serialize;

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
    let mut filters = Vec::new();
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
        .map(|&(_, kind, fc_hz, gain_db, q)| FilterSpec {
            kind,
            fc_hz,
            gain_db,
            q,
        })
        .collect();
    let sum_db = chain_response_db(&specs, preamp_db, FS, &freqs);

    let filter_dtos = filters
        .iter()
        .map(|&(enabled, kind, fc_hz, gain_db, q)| {
            let biquad = dsp::Biquad::rbj(kind, FS, fc_hz, gain_db, q);
            EqFilter {
                enabled,
                kind: kind.code(),
                fc_hz,
                gain_db,
                q,
                response_db: freqs.iter().map(|&f| biquad.magnitude_db(f, FS)).collect(),
            }
        })
        .collect();

    Ok(EqState {
        device_name: devices::default_render_device_name(),
        preamp_db,
        freqs,
        sum_db,
        filters: filter_dtos,
        source_files,
    })
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![apo_status, eq_state])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
