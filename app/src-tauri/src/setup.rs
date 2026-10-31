//! Settings › SETUP: "is my EQ reaching my ears?" (ADR-0018).
//!
//! Three parts. `setup_checks` reads the setup and names anything that would
//! keep APO from playing fletcher.txt on the default output. `path_test`
//! hears it: it changes fletcher.txt and measures the change on the output's
//! loopback (pathcheck.rs). `export_diagnostics` writes all of it, plus the
//! config folder and the device registry, to a zip the user can send.

use crate::{
    ab_side, active_preset, apo, data_dir, devices, eq_state, profile_running, write_fletcher_file,
    LAST_WRITTEN_LEVEL, TRIAL,
};
use fletcher_core::pathcheck;
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    /// "ok" | "warn" | "fail" | "info"
    status: &'static str,
    title: String,
    detail: String,
}

fn check(status: &'static str, title: impl Into<String>, detail: impl Into<String>) -> Check {
    Check {
        status,
        title: title.into(),
        detail: detail.into(),
    }
}

/// The key of a config line ("device" for `Device: …`), lowercase.
fn line_key(line: &str) -> Option<(String, &str)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (k, v) = line.split_once(':')?;
    Some((k.trim().to_ascii_lowercase(), v.trim()))
}

/// What applies where config.txt includes fletcher.txt. `Device:` and
/// `Stage:` selections end with the file they're in (measured:
/// docs/research/apo-scope-and-loopback.md), so only config.txt's own lines
/// before the Include matter.
struct IncludeSite {
    line: usize,
    /// The last `Device:` line before it that doesn't match this output.
    device_miss: Option<(usize, String)>,
    /// The `Stage:` selection in force, if any line set one.
    stage: Option<(usize, String)>,
}

fn include_site(config: &str, match_text: Option<&str>) -> Option<IncludeSite> {
    let mut device: Option<(usize, String)> = None;
    let mut stage: Option<(usize, String)> = None;
    for (i, line) in config.lines().enumerate() {
        let Some((key, value)) = line_key(line) else {
            continue;
        };
        match key.as_str() {
            "device" => device = Some((i + 1, value.to_string())),
            "stage" => stage = Some((i + 1, value.to_ascii_lowercase())),
            "include" if value.eq_ignore_ascii_case("fletcher.txt") => {
                let device_miss = device.filter(|(_, p)| {
                    match_text.is_some_and(|t| !apo::device_pattern_matches(p, t))
                });
                return Some(IncludeSite {
                    line: i + 1,
                    device_miss,
                    stage,
                });
            }
            _ => {}
        }
    }
    None
}

#[tauri::command]
pub fn setup_checks() -> Vec<Check> {
    let mut out = Vec::new();
    let install = match apo::detect() {
        Ok(i) => i,
        Err(e) => {
            out.push(check(
                "fail",
                "Equalizer APO isn't installed",
                e.to_string(),
            ));
            return out;
        }
    };
    out.push(check(
        "ok",
        format!(
            "Equalizer APO {}",
            apo::installed_version().unwrap_or_else(|| "(version unknown)".into())
        ),
        format!("config folder {}", install.config_path.display()),
    ));

    let name = devices::default_render_device_name().unwrap_or_else(|| "your output".into());
    let id = devices::default_render_device_id();
    let status = id.as_deref().map(apo::device_apo_status);
    out.push(match status {
        Some(apo::DeviceApo::Active) => check("ok", format!("APO is on {name}"), "your default output"),
        Some(apo::DeviceApo::Missing) => check(
            "fail",
            format!("APO isn't on {name}"),
            "Nothing Fletcher or Peace writes is heard on it. Open APO's Device Selector from ⚙ next to the device in the device menu, and tick it.",
        ),
        Some(apo::DeviceApo::SysFxDisabled) => check(
            "fail",
            format!("Enhancements are off on {name}"),
            "Windows' \"Disable all enhancements\" is on for this device, so APO doesn't run.",
        ),
        _ => check("warn", format!("Couldn't read {name}'s effects"), "Fletcher can't tell whether APO is on it."),
    });

    if let Some((pre, post)) = id.as_deref().and_then(apo::device_apo_stages) {
        out.push(match (pre, post) {
            (_, true) => check(
                "ok",
                "APO runs after mixing (post-mix)",
                if pre { "also before mixing (pre-mix)" } else { "" },
            ),
            (true, false) => check(
                "fail",
                "APO only runs before mixing (pre-mix) here",
                "APO plays filters after mixing unless a Stage: line says otherwise, so Fletcher's EQ isn't heard on this device. In APO's Device Selector, tick Troubleshooting options and install APO post-mix as well.",
            ),
            (false, false) => check("info", "No APO stage found on this device", ""),
        });
    }

    let config =
        std::fs::read_to_string(install.config_path.join("config.txt")).unwrap_or_default();
    let match_text = id.as_deref().and_then(apo::device_match_text);
    match include_site(&config, match_text.as_deref()) {
        None => out.push(check(
            "fail",
            "config.txt doesn't include fletcher.txt",
            "Another tool rewrote config.txt. Fletcher adds the line back on its next change: move any filter.",
        )),
        Some(site) => {
            out.push(check("ok", "config.txt includes fletcher.txt", format!("line {}", site.line)));
            if let Some((line, pattern)) = site.device_miss {
                out.push(check(
                    "fail",
                    format!("config.txt line {line} limits Fletcher to another device"),
                    format!(
                        "\"Device: {pattern}\" comes before Fletcher's Include and doesn't match {name}, so APO skips Fletcher's EQ on it. Move \"Include: fletcher.txt\" above it, or add \"Device: all\" just before it."
                    ),
                ));
            }
            if let Some((line, stages)) = site.stage.filter(|(_, s)| !s.contains("post-mix")) {
                out.push(check(
                    "warn",
                    format!("config.txt line {line} sets \"Stage: {stages}\""),
                    "Fletcher's EQ runs only in that stage.",
                ));
            }
        }
    }

    let copies: Vec<usize> = config
        .lines()
        .enumerate()
        .filter(|(_, l)| line_key(l).is_some_and(|(k, _)| k == "copy"))
        .map(|(i, _)| i + 1)
        .collect();
    if !copies.is_empty() {
        out.push(check(
            "info",
            "config.txt routes channels (Copy:)",
            format!(
                "line {}: routing can mix or replace channels after Fletcher's EQ",
                join(&copies)
            ),
        ));
    }
    out
}

fn join(lines: &[usize]) -> String {
    lines
        .iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PathResult {
    /// "post-mix" (works) | "pre-mix" (works only there) | "none" | "unmeasured"
    verdict: &'static str,
    /// How much a −20 dB test change lowered the tone, per stage tried.
    post_drop_db: Option<f64>,
    pre_drop_db: Option<f64>,
    detail: String,
}

/// Puts fletcher.txt back however the test ends.
struct RestoreFile<'a> {
    dir: &'a Path,
    text: &'a str,
}

impl Drop for RestoreFile<'_> {
    fn drop(&mut self) {
        let _ = write_fletcher_file(self.dir, self.text);
    }
}

/// How far `extra` (appended to fletcher.txt) lowers the tone, in the ear it
/// lowers least. Waits up to 4 s for APO to reload.
fn drop_with(dir: &Path, original: &str, extra: &str, base: f64) -> Result<f64, String> {
    let sep = if original.ends_with('\n') || original.is_empty() {
        ""
    } else {
        "\r\n"
    };
    write_fletcher_file(dir, &format!("{original}{sep}{extra}\r\n"))?;
    let start = std::time::Instant::now();
    let mut best = 0.0f64;
    while start.elapsed() < std::time::Duration::from_secs(4) {
        let (l, r) = pathcheck::tone_level_db(0.35)?;
        best = best.max(base - l.max(r));
        if best >= 15.0 {
            break;
        }
    }
    write_fletcher_file(dir, original)?;
    // Let the restore land before measuring anything else.
    std::thread::sleep(std::time::Duration::from_millis(1500));
    Ok(best)
}

fn path_test_sync() -> Result<PathResult, String> {
    if TRIAL.lock().unwrap().is_some() || profile_running() {
        return Err("finish the blind test or hearing session first".into());
    }
    let install = apo::detect().map_err(|e| e.to_string())?;
    let dir = install.config_path.as_path();
    // Holding the writer lock: no edit, fade or flip lands mid-test. Every
    // test write is the current file plus a cut, so nothing gets louder than
    // it already was.
    let _writer = LAST_WRITTEN_LEVEL.lock().unwrap();
    let original = std::fs::read_to_string(dir.join("fletcher.txt")).unwrap_or_default();
    let _restore = RestoreFile {
        dir,
        text: &original,
    };
    let unmeasured = |detail: String| PathResult {
        verdict: "unmeasured",
        post_drop_db: None,
        pre_drop_db: None,
        detail,
    };
    let _tone = match pathcheck::Tone::start() {
        Ok(t) => t,
        Err(e) => return Ok(unmeasured(format!("couldn't play the test tone: {e}"))),
    };
    std::thread::sleep(std::time::Duration::from_millis(400));
    let (l, r) = pathcheck::tone_level_db(0.5)?;
    let base = l.min(r);
    if base < -90.0 {
        return Ok(unmeasured(
            "the test tone didn't show up in the output's recording, so Fletcher can't measure this output".into(),
        ));
    }
    let post = drop_with(dir, &original, "Preamp: -20 dB", base)?;
    if post >= 15.0 {
        return Ok(PathResult {
            verdict: "post-mix",
            post_drop_db: Some(post),
            pre_drop_db: None,
            detail: "A −20 dB test change to Fletcher's file lowered the output as expected. Equalizer APO is playing Fletcher's EQ.".into(),
        });
    }
    let pre = drop_with(dir, &original, "Stage: pre-mix\r\nPreamp: -20 dB", base)?;
    Ok(if pre >= 15.0 {
        PathResult {
            verdict: "pre-mix",
            post_drop_db: Some(post),
            pre_drop_db: Some(pre),
            detail: "APO on this output only runs before mixing (pre-mix). Fletcher writes for after mixing, so its EQ isn't heard.".into(),
        }
    } else {
        PathResult {
            verdict: "none",
            post_drop_db: Some(post),
            pre_drop_db: Some(pre),
            detail: "A −20 dB test change to Fletcher's file didn't change the output at all. APO isn't playing Fletcher's file on this output.".into(),
        }
    })
}

#[tauri::command]
pub async fn path_test() -> Result<PathResult, String> {
    tauri::async_runtime::spawn_blocking(path_test_sync)
        .await
        .map_err(|e| e.to_string())?
}

// ---------------- diagnostics export ----------------

/// Programs and processes whose names suggest they touch audio. Matching is
/// by name only; a list of everything installed would mostly be noise and
/// more personal than a bug report needs.
const AUDIO_WORDS: &[&str] = &[
    "audio",
    "sound",
    "equalizer",
    "apo",
    "peace",
    "dolby",
    "dts",
    "nahimic",
    "sonic studio",
    "realtek",
    "voicemeeter",
    "vb-audio",
    "fxsound",
    "boom",
    "sonar",
    "steelseries",
    "wave link",
    "elgato",
    "razer",
    "synapse",
    "g hub",
    "lghub",
    "broadcast",
    "krisp",
    "eartrumpet",
    "soundswitch",
    "asio",
    "waves",
    "maxx",
    "sennheiser",
    "creative",
    "sound blaster",
    "armoury",
    "hyperx",
    "corsair",
    "icue",
    "jabra",
    "viper",
    "spatial",
    "atmos",
    "thx",
    "bongiovi",
    "conexant",
    "cirrus",
    "smart sound",
];

fn audioish(name: &str) -> bool {
    let n = name.to_lowercase();
    AUDIO_WORDS.iter().any(|w| n.contains(w))
}

fn installed_programs() -> Vec<String> {
    use winreg::enums::{
        HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY,
    };
    use winreg::RegKey;
    let mut out = Vec::new();
    let path = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall";
    for (hive, flags) in [
        (HKEY_LOCAL_MACHINE, KEY_READ | KEY_WOW64_64KEY),
        (HKEY_LOCAL_MACHINE, KEY_READ | KEY_WOW64_32KEY),
        (HKEY_CURRENT_USER, KEY_READ),
    ] {
        let Ok(root) = RegKey::predef(hive).open_subkey_with_flags(path, flags) else {
            continue;
        };
        for sub in root.enum_keys().filter_map(|k| k.ok()) {
            let Ok(k) = root.open_subkey_with_flags(&sub, flags) else {
                continue;
            };
            let Ok(name) = k.get_value::<String, _>("DisplayName") else {
                continue;
            };
            if audioish(&name) {
                let ver = k
                    .get_value::<String, _>("DisplayVersion")
                    .unwrap_or_default();
                out.push(format!("{name} {ver}").trim().to_string());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn running_processes() -> Vec<String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let Ok(o) = std::process::Command::new("tasklist")
        .args(["/fo", "csv", "/nh"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    else {
        return vec!["(tasklist unavailable)".into()];
    };
    let mut names: Vec<String> = String::from_utf8_lossy(&o.stdout)
        .lines()
        .filter_map(|l| l.split("\",\"").next())
        .map(|n| n.trim_matches('"').to_string())
        .filter(|n| audioish(n) || n.to_lowercase().starts_with("fletcher"))
        .collect();
    names.sort();
    names.dedup();
    names
}

fn windows_version() -> String {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    use winreg::RegKey;
    let Ok(k) = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion")
    else {
        return "unknown".into();
    };
    let s = |n: &str| k.get_value::<String, _>(n).unwrap_or_default();
    let ubr = k
        .get_value::<u32, _>("UBR")
        .map(|u| format!(".{u}"))
        .unwrap_or_default();
    format!(
        "{} {} (build {}{ubr})",
        s("ProductName"),
        s("DisplayVersion"),
        s("CurrentBuild")
    )
}

/// Files under `dir`, relative paths, skipping programs, icons and anything
/// over 2 MB (Peace keeps its installer and help in APO's config folder).
fn config_files(dir: &Path) -> Vec<(String, PathBuf)> {
    fn walk(root: &Path, dir: &Path, depth: u32, out: &mut Vec<(String, PathBuf)>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.filter_map(|e| e.ok()) {
            let p = e.path();
            if p.is_dir() {
                if depth < 3 {
                    walk(root, &p, depth + 1, out);
                }
                continue;
            }
            let ext = p
                .extension()
                .and_then(|x| x.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let big = e.metadata().map_or(true, |m| m.len() > 2_000_000);
            if big
                || [
                    "exe", "dll", "ico", "chm", "png", "jpg", "bmp", "wav", "msi",
                ]
                .contains(&ext.as_str())
            {
                continue;
            }
            let rel = p
                .strip_prefix(root)
                .unwrap_or(&p)
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, p));
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, 0, &mut out);
    out.sort();
    out
}

fn report_text(path_result: Option<&str>) -> String {
    let mut r = String::new();
    let mut line = |s: String| {
        r.push_str(&s);
        r.push_str("\r\n");
    };
    line(format!("Fletcher {}", env!("CARGO_PKG_VERSION")));
    line(format!("Windows: {}", windows_version()));
    let id = devices::default_render_device_id();
    line(format!(
        "Default output: {} [{}]",
        devices::default_render_device_name().unwrap_or_default(),
        id.clone().unwrap_or_default()
    ));
    if let Some(t) = id.as_deref().and_then(apo::device_match_text) {
        line(format!("APO's Device: match text: {t}"));
    }
    if let Some((pre, post)) = id.as_deref().and_then(apo::device_apo_stages) {
        line(format!("APO classes on it: pre-mix {pre}, post-mix {post}"));
    }
    line(format!(
        "Mix format: {}",
        devices::default_render_mix_format().unwrap_or_else(|| "unknown".into())
    ));
    line(String::new());
    line("FLETCHER'S VIEW (full state in fletcher/eq_state.json)".into());
    line(format!(
        "Active preset: {}",
        active_preset().unwrap_or_else(|| "none".into())
    ));
    line(format!("A/B side: {}", ab_side()));
    match eq_state() {
        Ok(s) => {
            let v = serde_json::to_value(&s).unwrap_or_default();
            line(format!(
                "Sources it counts as playing: {}",
                v["sourceFiles"]
            ));
            line(format!("Includes in config.txt: {}", v["includes"]));
            line(format!(
                "Preamp total {} dB, Fletcher's own {} dB, split {}",
                v["preampDb"], v["ownPreampDb"], v["split"]
            ));
        }
        Err(e) => line(format!("EQ state unreadable: {e}")),
    }
    line(String::new());
    line("CHECKS".into());
    for c in setup_checks() {
        line(format!("[{}] {}", c.status, c.title));
        if !c.detail.is_empty() {
            line(format!("       {}", c.detail));
        }
    }
    line(String::new());
    line("LIVE TEST".into());
    line(path_result.unwrap_or("not run").to_string());
    line(String::new());
    line("AUDIO SOFTWARE INSTALLED (by name)".into());
    for p in installed_programs() {
        line(format!("  {p}"));
    }
    line(String::new());
    line("AUDIO PROCESSES RUNNING (by name)".into());
    for p in running_processes() {
        line(format!("  {p}"));
    }
    r
}

fn describe(r: &PathResult) -> String {
    let db = |d: Option<f64>| d.map_or("—".into(), |d| format!("{d:.1} dB"));
    format!(
        "{}: {} (−20 dB test cut lowered the tone by: post-mix {}, pre-mix {})",
        r.verdict,
        r.detail,
        db(r.post_drop_db),
        db(r.pre_drop_db)
    )
}

/// Files that config files include from outside APO's config folder
/// (absolute or `..` paths), followed through nested includes.
fn outside_includes(config_dir: &Path) -> Vec<PathBuf> {
    let inside = |p: &Path| {
        p.canonicalize()
            .ok()
            .zip(config_dir.canonicalize().ok())
            .is_some_and(|(p, d)| p.starts_with(d))
    };
    let mut queue: Vec<PathBuf> = vec![config_dir.join("config.txt")];
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut outside = Vec::new();
    while let Some(file) = queue.pop() {
        if seen.contains(&file) || seen.len() > 200 {
            continue;
        }
        seen.push(file.clone());
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let dir = file.parent().unwrap_or(config_dir).to_path_buf();
        for line in text.lines() {
            if let Some((key, value)) = line_key(line) {
                if key == "include" {
                    let target = dir.join(value);
                    if target.is_file() {
                        if !inside(&target) && !outside.contains(&target) {
                            outside.push(target.clone());
                        }
                        queue.push(target);
                    }
                }
            }
        }
    }
    outside
}

fn export_sync(path: &Path, path_result: Option<String>) -> Result<(), String> {
    // The measurement is the most useful line in the report: run it if the
    // user exported without checking first.
    let path_result = path_result.or_else(|| {
        Some(match path_test_sync() {
            Ok(r) => describe(&r),
            Err(e) => format!("not run: {e}"),
        })
    });
    let file =
        std::fs::File::create(path).map_err(|e| format!("can't create {}: {e}", path.display()))?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut add = |name: &str, bytes: &[u8]| -> Result<(), String> {
        zip.start_file(name, opts).map_err(|e| e.to_string())?;
        zip.write_all(bytes).map_err(|e| e.to_string())
    };
    add("report.txt", report_text(path_result.as_deref()).as_bytes())?;
    add("registry/EqualizerAPO.txt", apo::registry_dump().as_bytes())?;
    add(
        "registry/playback-devices.txt",
        apo::render_endpoints_dump().as_bytes(),
    )?;
    if let Some(id) = devices::default_render_device_id() {
        add(
            "registry/default-output-properties.txt",
            apo::endpoint_properties_dump(&id).as_bytes(),
        )?;
    }
    if let Ok(install) = apo::detect() {
        for (rel, p) in config_files(&install.config_path) {
            if let Ok(bytes) = std::fs::read(&p) {
                add(&format!("apo-config/{rel}"), &bytes)?;
            }
        }
        for p in outside_includes(&install.config_path) {
            if let Ok(bytes) = std::fs::read(&p) {
                // The full path as a name: "C/Users/…/eq.txt".
                let name = p
                    .to_string_lossy()
                    .replace(['\\', ':'], "/")
                    .replace("//", "/");
                add(&format!("outside-includes/{name}"), &bytes)?;
            }
        }
    }
    if let Ok(state) = eq_state() {
        let json = serde_json::to_string_pretty(&state).unwrap_or_default();
        add("fletcher/eq_state.json", json.as_bytes())?;
    }
    let data = data_dir();
    if let Ok(bytes) = std::fs::read(data.join("state.json")) {
        add("fletcher/state.json", &bytes)?;
    }
    for (rel, p) in config_files(&data.join("presets")) {
        if let Ok(bytes) = std::fs::read(&p) {
            add(&format!("fletcher/presets/{rel}"), &bytes)?;
        }
    }
    zip.finish().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn export_diagnostics(path: String, path_result: Option<String>) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || export_sync(Path::new(&path), path_result))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODI: &str = "Schiit Modi 3+ Speakers {e411be18-23e9-460d-9df3-bb467280999e}";

    #[test]
    fn include_site_sees_device_scope_before_it() {
        let cfg = "Device: Realtek Headphones\r\nInclude: peace.txt\r\nInclude: fletcher.txt\r\n";
        let site = include_site(cfg, Some(MODI)).unwrap();
        assert_eq!(site.line, 3);
        assert_eq!(site.device_miss, Some((1, "Realtek Headphones".into())));

        let cfg =
            "Device: Realtek Headphones\nInclude: peace.txt\nDevice: all\nInclude: fletcher.txt\n";
        assert_eq!(include_site(cfg, Some(MODI)).unwrap().device_miss, None);

        let cfg = "Device: Modi\nInclude: fletcher.txt\n";
        assert_eq!(include_site(cfg, Some(MODI)).unwrap().device_miss, None);

        assert!(include_site("Include: peace.txt\n", Some(MODI)).is_none());
        // A commented-out Include doesn't count.
        assert!(include_site("# Include: fletcher.txt\n", Some(MODI)).is_none());
    }

    /// Machine-dependent: plays a faint tone on the default output, changes
    /// fletcher.txt for a few seconds, and writes a zip to %TEMP%. Run with
    /// `cargo test -p fletcher-app live_setup -- --ignored --nocapture`.
    #[test]
    #[ignore = "plays audio and rewrites fletcher.txt"]
    fn live_setup() {
        for c in setup_checks() {
            println!("[{}] {} — {}", c.status, c.title, c.detail);
        }
        let before =
            std::fs::read_to_string(apo::detect().unwrap().config_path.join("fletcher.txt"))
                .unwrap();
        let r = path_test_sync().unwrap();
        println!(
            "{} post {:?} pre {:?}: {}",
            r.verdict, r.post_drop_db, r.pre_drop_db, r.detail
        );
        let after =
            std::fs::read_to_string(apo::detect().unwrap().config_path.join("fletcher.txt"))
                .unwrap();
        assert_eq!(before, after, "fletcher.txt restored");
        let zip = std::env::temp_dir().join("fletcher-diagnostics-test.zip");
        export_sync(&zip, None).unwrap();
        println!(
            "zip {} bytes at {}",
            std::fs::metadata(&zip).unwrap().len(),
            zip.display()
        );
    }

    #[test]
    fn include_site_reads_stage() {
        let cfg = "Stage: pre-mix\nInclude: fletcher.txt\n";
        let site = include_site(cfg, Some(MODI)).unwrap();
        assert_eq!(site.stage, Some((1, "pre-mix".into())));
    }
}
