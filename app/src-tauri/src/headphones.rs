// Targets, measurements and preference bounds: the data side (ADR-0021).
//
// - An output's headphone is a measurement, bound in that output's settings
//   (state.json deviceSettings.<id>.headphone): AutoEq's raw measurement,
//   fetched on demand and cached (ADR-0008), or the user's own file(s).
// - Targets belong to no device: AutoEq's web-app set (featured), every file
//   in AutoEq's targets/ (searchable), and the user's own (a base plus four
//   knobs, or an imported file) in %APPDATA%\Fletcher\targets\.
// - `eq_overlay` lines all of it up with the live EQ for the graph's ghosts.
//
// AutoEq's layout is a runtime interface (ADR-0008): results paths read
// "<source>/<rig + form>/<name>", measurements live at
// "measurements/<source>/data/<form>/[<rig>/]<name>.csv", and the Rtings files
// are mostly in the older lowercase "rtings" folder. crinacle's measurements
// are no longer published there, so those entries say so instead of failing
// silently.

use crate::{data_dir, fsx, http_get, write_state_field};
use fletcher_core::response::{self, Bounds, Curve, Measurement, Smoothing, TargetKnobs};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const AUTOEQ_RAW: &str = "https://raw.githubusercontent.com/jaakkopasanen/AutoEq/master";
const AUTOEQ_TARGETS_API: &str =
    "https://api.github.com/repos/jaakkopasanen/AutoEq/contents/targets";
/// Indexes are refreshed weekly; measurement and target files never change
/// under their names, so they're cached for good.
const INDEX_TTL_SECS: u64 = 7 * 24 * 3600;
const FORMS: [&str; 3] = ["over-ear", "in-ear", "earbud"];

fn autoeq_cache() -> PathBuf {
    data_dir().join("autoeq")
}

/// A name made safe as one path component (cache files only).
fn file_safe(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    t.trim().trim_start_matches('.').chars().take(120).collect()
}

/// Percent-encode one URL path component.
fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn pct_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .filter(|_| bytes[i] == b'%')
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        if let Some(v) = hex {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Read a cached file if it's younger than `ttl`, else fetch and cache it;
/// on a network error, fall back to any cached copy (stale-if-error).
fn cached_fetch(path: &Path, url: &str, ttl: Option<u64>) -> Result<String, String> {
    let fresh = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| ttl.is_none_or(|ttl| age.as_secs() < ttl));
    if fresh {
        if let Ok(text) = std::fs::read_to_string(path) {
            return Ok(text);
        }
    }
    match http_get(url) {
        Ok(text) => {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = fsx::write_atomic(path, &text);
            Ok(text)
        }
        Err(e) => std::fs::read_to_string(path).map_err(|_| e),
    }
}

// ---------------- headphones (measurements) ----------------

/// One AutoEq measurement found by search.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HeadphoneHit {
    pub name: String,
    /// Who measured it ("oratory1990", "Rtings", …).
    pub source: String,
    pub form: String,
    /// The rig, when AutoEq's path names it ("HMS II.3", "GRAS 43AG-7", …).
    pub rig: Option<String>,
    /// AutoEq's results path, URL-encoded: "<source>/<rig + form>/<name>".
    pub path: String,
}

/// "<source>/<rig + form>/<name>" → (source, form, rig, name).
fn split_results_path(path: &str) -> Option<(String, String, Option<String>, String)> {
    let parts: Vec<String> = path.split('/').map(pct_decode).collect();
    let [source, middle, name] = parts.as_slice() else {
        return None;
    };
    let form = FORMS
        .iter()
        .find(|f| middle == *f || middle.ends_with(&format!(" {f}")))?;
    let rig = middle
        .strip_suffix(form)
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty());
    Some((source.clone(), form.to_string(), rig, name.clone()))
}

pub fn headphone_search_sync(query: &str) -> Result<Vec<HeadphoneHit>, String> {
    let q = query.trim().to_lowercase();
    if q.len() < 2 {
        return Ok(Vec::new());
    }
    let tokens: Vec<&str> = q.split_whitespace().collect();
    let index = crate::autoeq_index()?;
    Ok(crate::parse_autoeq_index(&index)
        .into_iter()
        .filter(|e| {
            let name = e.name.to_lowercase();
            tokens.iter().all(|t| name.contains(t))
        })
        .filter_map(|e| {
            let (source, form, rig, _) = split_results_path(&e.path)?;
            Some(HeadphoneHit {
                name: e.name,
                source,
                form,
                rig,
                path: e.path,
            })
        })
        .take(60)
        .collect())
}

#[tauri::command]
pub async fn headphone_search(query: String) -> Result<Vec<HeadphoneHit>, String> {
    tauri::async_runtime::spawn_blocking(move || headphone_search_sync(&query))
        .await
        .map_err(|e| e.to_string())?
}

/// The rig a source's own index gives a model (for sources whose paths don't
/// name it, e.g. oratory1990's "GRAS 45BC-10").
fn rig_from_name_index(source: &str, name: &str) -> Option<String> {
    let cache = autoeq_cache()
        .join("name_index")
        .join(format!("{}.tsv", file_safe(source)));
    let url = format!("{AUTOEQ_RAW}/measurements/{}/name_index.tsv", enc(source));
    let text = cached_fetch(&cache, &url, Some(INDEX_TTL_SECS)).ok()?;
    let mut lines = text.lines();
    let header: Vec<&str> = lines.next()?.split('\t').collect();
    let col = |n: &str| header.iter().position(|h| *h == n);
    let (name_i, source_name_i, rig_i) = (col("name")?, col("source_name"), col("rig")?);
    lines.find_map(|l| {
        let c: Vec<&str> = l.split('\t').collect();
        let model = c
            .get(name_i)
            .filter(|n| !n.is_empty())
            .or_else(|| source_name_i.and_then(|i| c.get(i)))?;
        (model.trim() == name)
            .then(|| c.get(rig_i).map(|r| r.trim().to_string()))
            .flatten()
            .filter(|r| !r.is_empty())
    })
}

/// Fetch (or read from cache) AutoEq's raw measurement for a results path.
fn fetch_autoeq_measurement(path: &str) -> Result<(HeadphoneHit, Curve), String> {
    let (source, form, rig, name) =
        split_results_path(path).ok_or("that AutoEq entry has an unexpected path")?;
    if name.contains("..") || source.contains("..") {
        return Err("invalid entry".into());
    }
    let cache = autoeq_cache()
        .join("measurements")
        .join(file_safe(&source))
        .join(file_safe(&form))
        .join(file_safe(rig.as_deref().unwrap_or("-")))
        .join(format!("{}.csv", file_safe(&name)));
    let text = match std::fs::read_to_string(&cache) {
        Ok(t) => t,
        Err(_) => {
            let mut sources = vec![source.clone()];
            if source.to_lowercase() != source {
                sources.push(source.to_lowercase());
            }
            let mut last_err = String::new();
            let mut found = None;
            for s in &sources {
                let mut rel = vec![enc(s), "data".into(), enc(&form)];
                if let Some(r) = &rig {
                    rel.push(enc(r));
                }
                rel.push(format!("{}.csv", enc(&name)));
                match http_get(&format!("{AUTOEQ_RAW}/measurements/{}", rel.join("/"))) {
                    Ok(t) => {
                        found = Some(t);
                        break;
                    }
                    Err(e) => last_err = e,
                }
            }
            let t = found.ok_or_else(|| {
                if last_err.contains("404") {
                    format!("AutoEq doesn't publish {source}'s measurement of {name}; try another source")
                } else {
                    last_err
                }
            })?;
            if let Some(dir) = cache.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = fsx::write_atomic(&cache, &t);
            t
        }
    };
    let curve = Curve::parse(&text)?;
    let rig = rig.or_else(|| rig_from_name_index(&source, &name));
    Ok((
        HeadphoneHit {
            name,
            source,
            form,
            rig,
            path: path.to_string(),
        },
        curve,
    ))
}

/// An output's headphone, as stored in its device settings.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HeadphoneBinding {
    pub name: String,
    /// Who measured it; "you" for an imported file.
    pub source: String,
    pub form: Option<String>,
    pub rig: Option<String>,
    /// AutoEq's results path, for an AutoEq measurement.
    pub autoeq: Option<String>,
    /// Imported files, relative to the data folder: left (or both), then right.
    #[serde(default)]
    pub files: Vec<String>,
}

pub fn headphone_of(device_id: &str) -> Option<HeadphoneBinding> {
    let v = crate::device_settings()
        .get(device_id)?
        .get("headphone")?
        .clone();
    serde_json::from_value(v).ok()
}

fn set_headphone(device_id: &str, h: Option<&HeadphoneBinding>) {
    let mut all = crate::device_settings();
    let entry = all
        .entry(device_id.to_string())
        .or_insert_with(|| serde_json::json!({}));
    match (h, entry.as_object_mut()) {
        (Some(h), _) => entry["headphone"] = serde_json::to_value(h).unwrap_or_default(),
        (None, Some(o)) => {
            o.remove("headphone");
        }
        _ => {}
    }
    write_state_field("deviceSettings", serde_json::Value::Object(all));
}

/// A bound headphone's measurement curve(s).
fn load_measurement(h: &HeadphoneBinding) -> Result<Measurement, String> {
    if let Some(path) = &h.autoeq {
        return Ok(Measurement {
            left: fetch_autoeq_measurement(path)?.1,
            right: None,
        });
    }
    let read = |rel: &String| -> Result<Curve, String> {
        let text = std::fs::read_to_string(data_dir().join(rel))
            .map_err(|e| format!("couldn't read the measurement file: {e}"))?;
        Curve::parse(&text)
    };
    let left = read(h.files.first().ok_or("no measurement file")?)?;
    let right = h.files.get(1).map(read).transpose()?;
    Ok(Measurement { left, right })
}

/// Bind an output to an AutoEq measurement (fetched now, so a missing one
/// says so here, not later on the graph).
#[tauri::command]
pub(crate) async fn device_headphone_autoeq(
    id: String,
    path: String,
) -> Result<Vec<crate::DeviceDto>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (hit, _) = fetch_autoeq_measurement(&path)?;
        crate::remember_device_name(&id);
        set_headphone(
            &id,
            Some(&HeadphoneBinding {
                name: hit.name,
                source: hit.source,
                form: Some(hit.form),
                rig: hit.rig,
                autoeq: Some(path),
                files: Vec::new(),
            }),
        );
        Ok(crate::devices_list())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Bind an output to the user's own measurement: one file, or one per ear
/// (left, right). The files are copied into the data folder unchanged (the
/// file itself is the record) after checking they read as a response.
#[tauri::command]
pub(crate) fn device_headphone_import(
    id: String,
    name: String,
    left: String,
    right: Option<String>,
) -> Result<Vec<crate::DeviceDto>, String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("give the headphone a name".into());
    }
    let dir_rel = format!("measurements/{}", file_safe(&name));
    let dir = data_dir().join(&dir_rel);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut files = Vec::new();
    for (src, ear) in std::iter::once((&left, "L")).chain(right.iter().map(|r| (r, "R"))) {
        let text = std::fs::read_to_string(src).map_err(|e| format!("{src}: {e}"))?;
        Curve::parse(&text).map_err(|e| format!("{src}: {e}"))?;
        let ext = Path::new(src)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("txt");
        let file = format!("{ear}.{ext}");
        fsx::write_atomic(&dir.join(&file), &text).map_err(|e| e.to_string())?;
        files.push(format!("{dir_rel}/{file}"));
    }
    crate::remember_device_name(&id);
    set_headphone(
        &id,
        Some(&HeadphoneBinding {
            name,
            source: "you".into(),
            form: None,
            rig: None,
            autoeq: None,
            files,
        }),
    );
    Ok(crate::devices_list())
}

#[tauri::command]
pub(crate) fn device_headphone_clear(id: String) -> Vec<crate::DeviceDto> {
    set_headphone(&id, None);
    crate::devices_list()
}

/// Every output with a headphone, connected or not, for Settings.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeadphoneRow {
    pub device_id: String,
    pub device_name: String,
    pub connected: bool,
    pub headphone: HeadphoneBinding,
}

#[tauri::command]
pub fn headphone_bindings() -> Vec<HeadphoneRow> {
    let connected = fletcher_core::devices::list_render_devices();
    crate::device_settings()
        .into_iter()
        .filter_map(|(id, s)| {
            let headphone: HeadphoneBinding =
                serde_json::from_value(s.get("headphone")?.clone()).ok()?;
            let live = connected.iter().find(|d| d.id == id);
            Some(HeadphoneRow {
                device_name: live
                    .map(|d| d.name.clone())
                    .or_else(|| s.get("name").and_then(|n| n.as_str()).map(String::from))
                    .unwrap_or_else(|| "An output".into()),
                connected: live.is_some(),
                device_id: id,
                headphone,
            })
        })
        .collect()
}

// ---------------- targets ----------------

/// Which target: a featured AutoEq one (its web-app set), any AutoEq target
/// file, or one of the user's.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TargetRef {
    Featured { label: String },
    Autoeq { file: String },
    User { name: String },
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TargetInfo {
    pub target: TargetRef,
    pub name: String,
    /// "featured" | "autoeq" | "yours"
    pub group: &'static str,
    /// The rig it's meant for, when known.
    pub rig: Option<String>,
    /// AutoEq's pairing: the measurement sources (and rigs) it suits.
    pub pairs_with: Vec<String>,
    /// That rig's own ear response (its diffuse field), as a target to show.
    pub rig_response: Option<TargetRef>,
}

/// AutoEq's web-app targets: label, curve (without bass), bass boost, pairing.
#[derive(Deserialize, Clone)]
struct WebTarget {
    label: String,
    #[serde(default)]
    compatible: Vec<Pairing>,
    #[serde(rename = "bassBoost")]
    bass_boost: Option<BassBoost>,
    fr: WebFr,
}
#[derive(Deserialize, Clone)]
struct Pairing {
    source: String,
    form: Option<String>,
    rig: Option<String>,
}
#[derive(Deserialize, Clone)]
struct BassBoost {
    gain: f64,
}
#[derive(Deserialize, Clone)]
struct WebFr {
    frequency: Vec<f64>,
    raw: Vec<f64>,
}

fn web_targets() -> Result<Vec<WebTarget>, String> {
    let text = cached_fetch(
        &autoeq_cache().join("webapp-targets.json"),
        &format!("{AUTOEQ_RAW}/webapp/data/targets.json"),
        Some(INDEX_TTL_SECS),
    )?;
    serde_json::from_str(&text).map_err(|e| format!("AutoEq's target list changed shape: {e}"))
}

/// The rig a target name implies, and that rig's diffuse-field file.
fn rig_of(name: &str) -> (Option<String>, Option<TargetRef>) {
    let n = name.to_lowercase();
    let df = |f: &str| Some(TargetRef::Autoeq { file: f.into() });
    if n.contains("5128") {
        (
            Some("Brüel & Kjær 5128".into()),
            df("Diffuse field 5128.csv"),
        )
    } else if n.contains("hms ii.3") {
        (Some("HEAD acoustics HMS II.3".into()), None)
    } else if n.contains("711") {
        (Some("IEC 60318-4 (711) coupler".into()), None)
    } else if n.contains("gras") || n.contains("kemar") || n.contains("harman over-ear") {
        (
            Some("GRAS (KEMAR)".into()),
            df("Diffuse field GRAS KEMAR.csv"),
        )
    } else {
        (None, None)
    }
}

fn pairing_text(p: &Pairing) -> String {
    let form = p
        .form
        .as_deref()
        .map(|f| format!(" {f}"))
        .unwrap_or_default();
    match &p.rig {
        Some(r) => format!("{}{form} on {r}", p.source),
        None => format!("{}{form}", p.source),
    }
}

fn targets_dir() -> PathBuf {
    data_dir().join("targets")
}

/// A user's target: a base (or flat) plus knobs, or an imported file.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UserTarget {
    pub name: String,
    /// None = flat.
    pub base: Option<TargetRef>,
    #[serde(default)]
    pub bass_db: f64,
    #[serde(default)]
    pub ear_db: f64,
    #[serde(default)]
    pub treble_db: f64,
    #[serde(default)]
    pub tilt_db_per_oct: f64,
    /// An imported response file in the targets folder (instead of a base).
    pub file: Option<String>,
    /// Imported preference bounds: upper and lower offset files.
    pub bounds: Option<[String; 2]>,
}

impl UserTarget {
    fn knobs(&self) -> TargetKnobs {
        TargetKnobs {
            bass_db: self.bass_db,
            ear_db: self.ear_db,
            treble_db: self.treble_db,
            tilt_db_per_oct: self.tilt_db_per_oct,
        }
    }
}

fn user_target_path(name: &str) -> PathBuf {
    targets_dir().join(format!("{}.json", file_safe(name)))
}

fn user_targets() -> Vec<UserTarget> {
    let mut out: Vec<UserTarget> = std::fs::read_dir(targets_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok())
        .collect();
    out.sort_by_key(|t: &UserTarget| t.name.to_lowercase());
    out
}

fn user_target(name: &str) -> Option<UserTarget> {
    serde_json::from_str(&std::fs::read_to_string(user_target_path(name)).ok()?).ok()
}

/// Every AutoEq target file name, from GitHub's listing (cached weekly).
fn autoeq_target_files() -> Result<Vec<String>, String> {
    let text = cached_fetch(
        &autoeq_cache().join("targets-index.json"),
        AUTOEQ_TARGETS_API,
        Some(INDEX_TTL_SECS),
    )?;
    let items: Vec<serde_json::Value> =
        serde_json::from_str(&text).map_err(|e| format!("AutoEq's target listing: {e}"))?;
    Ok(items
        .iter()
        .filter_map(|i| i.get("name")?.as_str().map(String::from))
        .filter(|n| n.ends_with(".csv"))
        .collect())
}

/// A target's curve, resolved (a user target's base is resolved in turn).
pub fn target_curve(t: &TargetRef) -> Result<Curve, String> {
    target_curve_depth(t, 0)
}

/// AutoEq's curves, resolved once per run: they never change under their
/// names, and re-reading the web-app list (~450 KB) on every slider move made
/// the target editor lag. The user's own targets aren't cached; they change.
static AUTOEQ_CURVES: std::sync::Mutex<Option<std::collections::HashMap<String, Curve>>> =
    std::sync::Mutex::new(None);

fn target_curve_depth(t: &TargetRef, depth: usize) -> Result<Curve, String> {
    if depth > 4 {
        return Err("that target's base refers back to itself".into());
    }
    if matches!(t, TargetRef::User { .. }) {
        return resolve_target(t, depth);
    }
    let key = serde_json::to_string(t).unwrap_or_default();
    if let Some(c) = AUTOEQ_CURVES
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|m| m.get(&key))
    {
        return Ok(c.clone());
    }
    let c = resolve_target(t, depth)?;
    AUTOEQ_CURVES
        .lock()
        .unwrap()
        .get_or_insert_with(Default::default)
        .insert(key, c.clone());
    Ok(c)
}

fn resolve_target(t: &TargetRef, depth: usize) -> Result<Curve, String> {
    match t {
        TargetRef::Featured { label } => {
            let w = web_targets()?
                .into_iter()
                .find(|w| &w.label == label)
                .ok_or_else(|| format!("AutoEq no longer lists {label}"))?;
            // AutoEq stores these without bass and adds its bass boost.
            let base = Curve {
                freqs: w.fr.frequency,
                db: w.fr.raw,
            };
            Ok(TargetKnobs {
                bass_db: w.bass_boost.map_or(0.0, |b| b.gain),
                ..Default::default()
            }
            .apply(&base))
        }
        TargetRef::Autoeq { file } => {
            if file.contains(['/', '\\']) || file.contains("..") {
                return Err("invalid target file".into());
            }
            let text = cached_fetch(
                &autoeq_cache().join("targets").join(file_safe(file)),
                &format!("{AUTOEQ_RAW}/targets/{}", enc(file)),
                None,
            )?;
            Curve::parse(&text)
        }
        TargetRef::User { name } => {
            let u = user_target(name).ok_or_else(|| format!("no target named {name}"))?;
            let base = match (&u.file, &u.base) {
                (Some(f), _) => Curve::parse(
                    &std::fs::read_to_string(targets_dir().join(f)).map_err(|e| e.to_string())?,
                )?,
                (None, Some(b)) => target_curve_depth(b, depth + 1)?,
                (None, None) => {
                    let freqs = response::autoeq_grid();
                    Curve {
                        db: vec![0.0; freqs.len()],
                        freqs,
                    }
                }
            };
            Ok(u.knobs().apply(&base))
        }
    }
}

fn target_bounds(t: &TargetRef) -> Option<Bounds> {
    let TargetRef::User { name } = t else {
        return None;
    };
    let [upper, lower] = user_target(name)?.bounds?;
    let read =
        |f: &String| Curve::parse(&std::fs::read_to_string(targets_dir().join(f)).ok()?).ok();
    Some(Bounds {
        upper: read(&upper)?,
        lower: read(&lower)?,
    })
}

fn info_of_user(u: &UserTarget) -> TargetInfo {
    let (rig, rig_response) = match &u.base {
        Some(TargetRef::Autoeq { file }) => rig_of(file),
        Some(TargetRef::Featured { label }) => rig_of(label),
        _ => (None, None),
    };
    TargetInfo {
        target: TargetRef::User {
            name: u.name.clone(),
        },
        name: u.name.clone(),
        group: "yours",
        rig,
        pairs_with: Vec::new(),
        rig_response,
    }
}

/// The library for Settings: AutoEq's featured set and the user's own.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetLibrary {
    pub featured: Vec<TargetInfo>,
    pub yours: Vec<TargetInfo>,
    /// Why the featured set is empty, if it is (offline with no cache).
    pub featured_error: Option<String>,
}

pub fn targets_sync() -> TargetLibrary {
    let (featured, featured_error) = match web_targets() {
        Ok(ws) => (
            ws.iter()
                .map(|w| {
                    let (rig, rig_response) = rig_of(&w.label);
                    TargetInfo {
                        target: TargetRef::Featured {
                            label: w.label.clone(),
                        },
                        name: w.label.clone(),
                        group: "featured",
                        rig,
                        pairs_with: w.compatible.iter().map(pairing_text).collect(),
                        rig_response,
                    }
                })
                .collect(),
            None,
        ),
        Err(e) => (Vec::new(), Some(e)),
    };
    TargetLibrary {
        featured,
        yours: user_targets().iter().map(info_of_user).collect(),
        featured_error,
    }
}

#[tauri::command]
pub async fn targets() -> Result<TargetLibrary, String> {
    tauri::async_runtime::spawn_blocking(targets_sync)
        .await
        .map_err(|e| e.to_string())
}

/// Search every AutoEq target file by name ("it's just data").
#[tauri::command]
pub async fn targets_search(query: String) -> Result<Vec<TargetInfo>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let q = query.trim().to_lowercase();
        let tokens: Vec<&str> = q.split_whitespace().collect();
        Ok(autoeq_target_files()?
            .into_iter()
            .filter(|f| {
                let n = f.to_lowercase();
                tokens.iter().all(|t| n.contains(t))
            })
            .map(|file| {
                let (rig, rig_response) = rig_of(&file);
                TargetInfo {
                    name: file.trim_end_matches(".csv").to_string(),
                    target: TargetRef::Autoeq { file },
                    group: "autoeq",
                    rig,
                    pairs_with: Vec::new(),
                    rig_response,
                }
            })
            .collect())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// A target's curve on the graph's frequencies, anchored at 0 dB at 500 Hz
/// (for the editor's preview and the chooser).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurveDto {
    pub freqs: Vec<f64>,
    pub db: Vec<Option<f64>>,
    /// The preference band around it, when asked for (editor preview).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upper: Option<Vec<Option<f64>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lower: Option<Vec<Option<f64>>>,
}

/// Which bounds the editor's preview draws: a saved target's, or two files
/// just picked (not saved yet).
#[derive(Deserialize, Debug)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum BoundsSource {
    Saved { name: String },
    Files { paths: [String; 2] },
}

fn graph_freqs() -> Vec<f64> {
    fletcher_core::dsp::log_freqs(crate::CURVE_FMIN, 20000.0, crate::CURVE_POINTS)
}

/// A target's curve; with `knobs` (from the editor, unsaved), those knobs on
/// `base` instead; with `bounds`, the band around it too.
#[tauri::command]
pub async fn target_preview(
    target: Option<TargetRef>,
    base: Option<TargetRef>,
    knobs: Option<UserTarget>,
    bounds: Option<BoundsSource>,
) -> Result<CurveDto, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let curve = match (target, knobs) {
            (Some(t), None) => target_curve(&t)?,
            (_, Some(k)) => {
                let b = match &base {
                    Some(b) => target_curve(b)?,
                    None => {
                        let freqs = response::autoeq_grid();
                        Curve {
                            db: vec![0.0; freqs.len()],
                            freqs,
                        }
                    }
                };
                k.knobs().apply(&b)
            }
            (None, None) => return Err("no target".into()),
        };
        let placed = curve.shifted(response::anchor_offset(&curve).unwrap_or(0.0));
        let freqs = graph_freqs();
        let band = match bounds {
            Some(BoundsSource::Saved { name }) => target_bounds(&TargetRef::User { name }),
            Some(BoundsSource::Files { paths }) => {
                let [(_, upper), (_, lower)] = read_bounds_pair(&paths)?;
                Some(Bounds { upper, lower })
            }
            None => None,
        };
        let edge = |e: &Curve| -> Vec<Option<f64>> {
            freqs
                .iter()
                .map(|&f| Some(placed.at(f)? + e.at(f)?))
                .collect()
        };
        Ok(CurveDto {
            db: placed.sample(&freqs),
            upper: band.as_ref().map(|b| edge(&b.upper)),
            lower: band.as_ref().map(|b| edge(&b.lower)),
            freqs,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// What saving does to a target's preference bounds.
#[derive(Deserialize, Debug, Default)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum BoundsEdit {
    /// Keep the ones it has (none for a new target).
    #[default]
    Keep,
    /// Take these two files (either order: the data says which edge is which).
    Files { paths: [String; 2] },
    /// Drop them.
    Clear,
}

/// The name of a file a target owns in the targets folder: "<name><suffix>.txt".
fn owned_file(name: &str, suffix: &str) -> String {
    format!("{}{suffix}.txt", file_safe(name))
}

const UPPER: &str = " bounds upper";
const LOWER: &str = " bounds lower";

/// Two bounds files read and ordered upper first. Which is the upper edge is
/// read from the data, not the order they were picked in: the higher average.
fn read_bounds_pair(paths: &[String; 2]) -> Result<[(String, Curve); 2], String> {
    let read = |p: &String| -> Result<(String, Curve), String> {
        let text = std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"))?;
        let c = Curve::parse(&text).map_err(|e| format!("{p}: {e}"))?;
        Ok((text, c))
    };
    let (a, b) = (read(&paths[0])?, read(&paths[1])?);
    let mean = |c: &Curve| c.db.iter().sum::<f64>() / c.db.len().max(1) as f64;
    Ok(if mean(&a.1) >= mean(&b.1) {
        [a, b]
    } else {
        [b, a]
    })
}

/// Save one of the user's targets (create, or update; `previous` renames).
/// The files a target owns (an imported curve, its bounds) come from what's
/// on disk, never from the page, and are renamed along with the target.
#[tauri::command]
pub fn target_save(
    target: UserTarget,
    previous: Option<String>,
    bounds: Option<BoundsEdit>,
) -> Result<TargetLibrary, String> {
    let old = match &previous {
        Some(p) => Some(user_target(p).ok_or_else(|| format!("no target named {p}"))?),
        None => None,
    };
    save_user_target(
        UserTarget {
            file: old.as_ref().and_then(|o| o.file.clone()),
            bounds: old.as_ref().and_then(|o| o.bounds.clone()),
            ..target
        },
        previous.as_deref(),
        bounds.unwrap_or_default(),
    )
}

fn save_user_target(
    target: UserTarget,
    previous: Option<&str>,
    edit: BoundsEdit,
) -> Result<TargetLibrary, String> {
    let name =
        fletcher_core::presets::sanitize_name(&target.name).ok_or("give the target a name")?;
    if let Some(TargetRef::User { name: b }) = &target.base {
        if b == &name {
            return Err("a target can't be its own base".into());
        }
    }
    let renaming = previous.filter(|p| *p != name);
    if (renaming.is_some() || previous.is_none()) && user_target(&name).is_some() {
        return Err(format!("a target named {name:?} already exists"));
    }
    let dir = targets_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // A file that's already named for this target stays; one named for the
    // old name moves.
    let follow = |f: &String, suffix: &str| -> Result<String, String> {
        let to = owned_file(&name, suffix);
        if *f != to {
            std::fs::rename(dir.join(f), dir.join(&to)).map_err(|e| format!("{f}: {e}"))?;
        }
        Ok(to)
    };
    let file = target.file.as_ref().map(|f| follow(f, "")).transpose()?;
    let bounds = match (edit, &target.bounds) {
        (BoundsEdit::Keep, Some([u, l])) => Some([follow(u, UPPER)?, follow(l, LOWER)?]),
        (BoundsEdit::Keep, None) => None,
        (BoundsEdit::Files { paths }, old) => {
            let [(ut, _), (lt, _)] = read_bounds_pair(&paths)?;
            let (u, l) = (owned_file(&name, UPPER), owned_file(&name, LOWER));
            fsx::write_atomic(&dir.join(&u), &ut).map_err(|e| e.to_string())?;
            fsx::write_atomic(&dir.join(&l), &lt).map_err(|e| e.to_string())?;
            for f in old.iter().flatten().filter(|f| **f != u && **f != l) {
                let _ = std::fs::remove_file(dir.join(f));
            }
            Some([u, l])
        }
        (BoundsEdit::Clear, old) => {
            for f in old.iter().flatten() {
                let _ = std::fs::remove_file(dir.join(f));
            }
            None
        }
    };
    let target = UserTarget {
        name: name.clone(),
        file,
        bounds,
        ..target
    };
    let text = serde_json::to_string_pretty(&target).map_err(|e| e.to_string())?;
    fsx::write_atomic(&user_target_path(&name), &text).map_err(|e| e.to_string())?;
    if let Some(old) = renaming {
        let _ = std::fs::remove_file(user_target_path(old));
    }
    Ok(targets_sync())
}

/// Import a target file (and optionally preference bounds: upper and lower
/// offset files). The files are copied unchanged.
#[tauri::command]
pub fn target_import(
    name: String,
    path: String,
    bounds: Option<[String; 2]>,
) -> Result<TargetLibrary, String> {
    let name = fletcher_core::presets::sanitize_name(&name).ok_or("give the target a name")?;
    if user_target(&name).is_some() {
        return Err(format!("a target named {name:?} already exists"));
    }
    std::fs::create_dir_all(targets_dir()).map_err(|e| e.to_string())?;
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
    Curve::parse(&text).map_err(|e| format!("{path}: {e}"))?;
    let file = owned_file(&name, "");
    fsx::write_atomic(&targets_dir().join(&file), &text).map_err(|e| e.to_string())?;
    save_user_target(
        UserTarget {
            name,
            base: None,
            bass_db: 0.0,
            ear_db: 0.0,
            treble_db: 0.0,
            tilt_db_per_oct: 0.0,
            file: Some(file),
            bounds: None,
        },
        None,
        bounds.map_or(BoundsEdit::Keep, |paths| BoundsEdit::Files { paths }),
    )
}

/// The user's targets in full, for the editor.
#[tauri::command]
pub fn targets_user() -> Vec<UserTarget> {
    user_targets()
}

#[tauri::command]
pub fn target_delete(name: String) -> Result<TargetLibrary, String> {
    let u = user_target(&name).ok_or_else(|| format!("no target named {name}"))?;
    for f in u.file.iter().chain(u.bounds.iter().flatten()) {
        let _ = std::fs::remove_file(targets_dir().join(f));
    }
    std::fs::remove_file(user_target_path(&name)).map_err(|e| e.to_string())?;
    Ok(targets_sync())
}

// ---------------- the fit ("A target…", ADR-0021 step 6) ----------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FitResult {
    preset: String,
    filters: usize,
    state: crate::EqState,
}

/// The filters as a preset stores them, rounded the way AutoEq prints its
/// results (Hz to 1, gain to 0.1 dB, Q to 0.01).
fn fitted_chain(
    m: &Curve,
    target: &Curve,
) -> Result<Vec<fletcher_core::config::ChainFilter>, String> {
    Ok(fletcher_core::fit::fit(m, target)?
        .filters
        .into_iter()
        .map(|s| fletcher_core::config::ChainFilter {
            enabled: true,
            kind: s.kind,
            fc_hz: s.fc_hz.round(),
            gain_db: (s.gain_db * 10.0).round() / 10.0,
            q: (s.q * 100.0).round() / 100.0,
        })
        .collect())
}

/// Fit the playing output's headphone to `target` (AutoEq's method,
/// `fletcher_core::fit`) and save the filters as a new preset, which then
/// plays. Each ear gets its own fit when the measurement has both and each
/// ear's own EQ is on (ADR-0017); otherwise their average.
#[tauri::command]
pub(crate) async fn target_fit(target: TargetRef, name: String) -> Result<FitResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let device = fletcher_core::devices::list_render_devices()
            .into_iter()
            .find(|d| d.is_default)
            .ok_or("no output is the Windows default right now")?;
        let h = headphone_of(&device.id).ok_or_else(|| {
            format!(
                "{} has no headphone set: choose one in its settings",
                device.name
            )
        })?;
        let m = load_measurement(&h)?;
        let t = target_curve(&target)?;
        let chains = match (&m.right, crate::per_ear()) {
            (Some(right), true) => fletcher_core::config::Chains {
                left: fitted_chain(&m.left, &t)?,
                right: Some(fitted_chain(right, &t)?),
            },
            _ => fletcher_core::config::Chains::linked(fitted_chain(&m.average(), &t)?),
        };
        let specs = |c: &[fletcher_core::config::ChainFilter]| crate::filter_specs(c);
        let stereo = fletcher_core::dsp::StereoSpecs {
            left: specs(&chains.left),
            right: specs(chains.right.as_deref().unwrap_or(&chains.left)),
        };
        let filters = chains.left.len();
        let preset = crate::import_chains_as_preset(&name, crate::preset_preamp(&stereo), &chains)?;
        Ok(FitResult {
            preset,
            filters,
            state: crate::eq_state()?,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

// ---------------- the overlay ----------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlayDto {
    /// The EQ graph's own frequencies (eq_state's).
    pub freqs: Vec<f64>,
    /// The default output's headphone, if it has one.
    pub headphone: Option<HeadphoneBinding>,
    /// Why the measurement couldn't be loaded, if it couldn't.
    pub measurement_error: Option<String>,
    pub measurement_left: Vec<Option<f64>>,
    pub measurement_right: Option<Vec<Option<f64>>>,
    pub measurement_average: Vec<Option<f64>>,
    pub predicted_left: Vec<Option<f64>>,
    pub predicted_right: Option<Vec<Option<f64>>>,
    pub target: Vec<Option<f64>>,
    pub target_error: Option<String>,
    pub bounds_upper: Vec<Option<f64>>,
    pub bounds_lower: Vec<Option<f64>>,
    /// Where doubt about the measurement starts (shading fades in here) and
    /// where it's full: reliability fades, it doesn't stop.
    pub doubt_from_hz: f64,
    pub doubt_full_hz: f64,
}

fn parse_smoothing(s: &str) -> Smoothing {
    match s {
        "none" => Smoothing::None,
        "erb" => Smoothing::Erb,
        other => other
            .strip_prefix("1/")
            .and_then(|n| n.parse::<f64>().ok())
            .filter(|n| *n > 0.0)
            .map_or(Smoothing::Octave(12.0), Smoothing::Octave),
    }
}

/// The ghosts for the EQ graph: the default output's headphone measurement,
/// measurement + the live EQ (per ear, as the graph draws it), the chosen target
/// fitted to the measurement, and its bounds. `smoothing`: "none", "1/24",
/// "1/12" (default), "1/6", "1/3", "erb".
#[tauri::command]
pub async fn eq_overlay(
    target: Option<TargetRef>,
    smoothing: Option<String>,
    robust: Option<bool>,
) -> Result<OverlayDto, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let eq = crate::eq_state()?;
        let headphone = fletcher_core::devices::list_render_devices()
            .into_iter()
            .find(|d| d.is_default)
            .and_then(|d| headphone_of(&d.id));
        let (measurement, measurement_error) = match &headphone {
            Some(h) => match load_measurement(h) {
                Ok(m) => (Some(m), None),
                Err(e) => (None, Some(e)),
            },
            None => (None, None),
        };
        let (target_curve_v, target_error) = match &target {
            Some(t) => match target_curve(t) {
                Ok(c) => (Some(c), None),
                Err(e) => (None, Some(e)),
            },
            None => (None, None),
        };
        let bounds = target.as_ref().and_then(target_bounds);
        // The EQ as the graph draws it: without the preamp. The preamp is a
        // level, which level matching handles; drawn in, it would push the
        // predicted curve below a target it matches in shape.
        let drawn = |sum: &[f64]| -> Vec<f64> { sum.iter().map(|v| v - eq.preamp_db).collect() };
        let eq_left = drawn(&eq.sum_db);
        let eq_right = eq.sum_db_right.as_deref().map(drawn);
        let o = response::overlay(&response::OverlayInput {
            freqs: &eq.freqs,
            measurement: measurement.as_ref(),
            target: target_curve_v.as_ref(),
            bounds: bounds.as_ref(),
            smoothing: parse_smoothing(smoothing.as_deref().unwrap_or("1/12")),
            eq_left: &eq_left,
            eq_right: eq_right.as_deref(),
            robust_fit: robust.unwrap_or(false),
        });
        Ok(OverlayDto {
            freqs: eq.freqs,
            headphone: headphone.clone(),
            measurement_error,
            measurement_left: o.measurement_left,
            measurement_right: o.measurement_right,
            measurement_average: o.measurement_average,
            predicted_left: o.predicted_left,
            predicted_right: o.predicted_right,
            target: o.target,
            target_error,
            bounds_upper: o.bounds_upper,
            bounds_lower: o.bounds_lower,
            doubt_from_hz: response::doubt_from_hz(
                headphone.as_ref().and_then(|h| h.form.as_deref()),
            ),
            doubt_full_hz: response::DOUBT_FULL_HZ,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_paths_split_into_source_form_rig_name() {
        assert_eq!(
            split_results_path("oratory1990/over-ear/Sennheiser%20HD%20650"),
            Some((
                "oratory1990".into(),
                "over-ear".into(),
                None,
                "Sennheiser HD 650".into()
            ))
        );
        assert_eq!(
            split_results_path("Rtings/HMS%20II.3%20over-ear/Sennheiser%20HD%20650"),
            Some((
                "Rtings".into(),
                "over-ear".into(),
                Some("HMS II.3".into()),
                "Sennheiser HD 650".into()
            ))
        );
        assert_eq!(
            split_results_path("crinacle/EARS%20%2B%20711%20over-ear/X"),
            Some((
                "crinacle".into(),
                "over-ear".into(),
                Some("EARS + 711".into()),
                "X".into()
            ))
        );
        assert_eq!(split_results_path("a/b"), None);
        assert_eq!(split_results_path("a/sideways/x"), None);
    }

    #[test]
    fn url_components_are_encoded() {
        assert_eq!(enc("Bruel & Kjaer 5128"), "Bruel%20%26%20Kjaer%205128");
        assert_eq!(
            enc("1MORE Aero (ANC Off)"),
            "1MORE%20Aero%20%28ANC%20Off%29"
        );
        assert_eq!(pct_decode("EARS%20%2B%20711"), "EARS + 711");
        assert_eq!(pct_decode("100%"), "100%");
    }

    #[test]
    fn smoothing_names() {
        assert_eq!(parse_smoothing("none"), Smoothing::None);
        assert_eq!(parse_smoothing("1/6"), Smoothing::Octave(6.0));
        assert_eq!(parse_smoothing("erb"), Smoothing::Erb);
        assert_eq!(parse_smoothing("nonsense"), Smoothing::Octave(12.0));
    }

    #[test]
    fn rigs_and_their_ear_responses() {
        assert_eq!(
            rig_of("Diffuse field 5128 -1dB per octave.csv")
                .0
                .as_deref(),
            Some("Brüel & Kjær 5128")
        );
        assert!(rig_of("Harman over-ear 2018").1.is_some());
        assert_eq!(rig_of("Harman in-ear 2019").0, None);
    }

    #[test]
    #[ignore = "this machine: reads the real APO config and state"]
    fn live_overlay_on_this_machine() {
        let o = tauri::async_runtime::block_on(eq_overlay(
            Some(TargetRef::Featured {
                label: "Harman over-ear 2018".into(),
            }),
            Some("1/12".into()),
            None,
        ));
        match o {
            Ok(o) => println!(
                "headphone={:?} meas_err={:?} target_err={:?} meas_pts={} target_pts={}",
                o.headphone.map(|h| h.name),
                o.measurement_error,
                o.target_error,
                o.measurement_left.iter().flatten().count(),
                o.target.iter().flatten().count()
            ),
            Err(e) => println!("ERROR: {e}"),
        }
    }

    /// The fit as "A target…" runs it, minus saving the preset: this
    /// machine's output headphone and AutoEq's featured Harman target.
    #[test]
    #[ignore = "this machine: reads the real device binding; network"]
    fn live_fit_on_this_machine() {
        let device = fletcher_core::devices::list_render_devices()
            .into_iter()
            .find(|d| d.is_default)
            .expect("a default output");
        let h = headphone_of(&device.id).expect("a headphone on the default output");
        let m = load_measurement(&h).unwrap();
        let t = target_curve(&TargetRef::Featured {
            label: "Harman over-ear 2018".into(),
        })
        .unwrap();
        let chain = fitted_chain(&m.average(), &t).unwrap();
        println!("{} on {}:", h.name, device.name);
        for f in &chain {
            println!(
                "  {:?} {} Hz {:+.1} dB Q {:.2}",
                f.kind, f.fc_hz, f.gain_db, f.q
            );
        }
        assert_eq!(chain.len(), 10);
    }

    #[test]
    #[ignore = "network: fetches real AutoEq data"]
    fn live_autoeq_measurement_and_targets() {
        let (hit, curve) =
            fetch_autoeq_measurement("Rtings/HMS%20II.3%20over-ear/Sennheiser%20HD%20650").unwrap();
        assert_eq!(hit.rig.as_deref(), Some("HMS II.3"));
        assert!(curve.freqs.len() > 600);
        let (hit, _) =
            fetch_autoeq_measurement("oratory1990/over-ear/Sennheiser%20HD%20650").unwrap();
        assert!(hit.rig.is_some(), "rig from oratory1990's name index");
        let err =
            fetch_autoeq_measurement("crinacle/GRAS%2043AG-7%20over-ear/Sennheiser%20HD%20650")
                .unwrap_err();
        assert!(err.contains("doesn't publish"), "{err}");
        let lib = targets_sync();
        assert!(
            lib.featured
                .iter()
                .any(|t| t.name == "Harman over-ear 2018"),
            "{:?}",
            lib.featured_error
        );
        let harman = target_curve(&TargetRef::Featured {
            label: "Harman over-ear 2018".into(),
        })
        .unwrap();
        let file = target_curve(&TargetRef::Autoeq {
            file: "Harman over-ear 2018.csv".into(),
        })
        .unwrap();
        // The featured one is "without bass" + AutoEq's 6 dB bass boost.
        assert!((harman.at(30.0).unwrap() - file.at(30.0).unwrap()).abs() < 0.1);
        assert!(autoeq_target_files().unwrap().len() > 40);
    }
}
