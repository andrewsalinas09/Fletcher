//! Managed external tools: ffmpeg (the native decode path) and yt-dlp (URL
//! import). Fetched on demand with explicit consent — never bundled — per the
//! ADR-0008 precedent; GPL binaries invoked as separate processes keep the
//! Apache-2.0 codebase clean. Located on PATH first, else in
//! `%APPDATA%\Fletcher\tools\`.

use std::path::{Path, PathBuf};

const YTDLP_URL: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe";
// BtbN's canonical Windows builds; the zip carries ffmpeg.exe + ffprobe.exe.
const FFMPEG_URL: &str =
    "https://github.com/BtbN/FFmpeg-Builds/releases/latest/download/ffmpeg-master-latest-win64-gpl.zip";

pub fn tools_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("tools")
}

/// Locate a tool: managed copy first (predictable version), then PATH.
pub fn find_tool(data_dir: &Path, name: &str) -> Option<PathBuf> {
    let managed = tools_dir(data_dir).join(format!("{name}.exe"));
    if managed.is_file() {
        return Some(managed);
    }
    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var)
        .map(|dir| dir.join(format!("{name}.exe")))
        .find(|p| p.is_file())
}

fn download(
    url: &str,
    dest: &Path,
    mut on_progress: impl FnMut(u64, Option<u64>),
) -> Result<(), String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("Fletcher-EQ (github.com/andrewsalinas09/Fletcher)")
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())?;
    let mut resp = client
        .get(url)
        .send()
        .map_err(|e| format!("download failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("download failed ({}) for {url}", resp.status()));
    }
    let total = resp.content_length();
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = dest.with_extension("download");
    {
        let mut file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        let mut buf = [0u8; 64 * 1024];
        let mut done: u64 = 0;
        use std::io::{Read, Write};
        loop {
            let n = resp
                .read(&mut buf)
                .map_err(|e| format!("download interrupted: {e}"))?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            done += n as u64;
            on_progress(done, total);
        }
    }
    std::fs::rename(&tmp, dest).map_err(|e| e.to_string())?;
    Ok(())
}

/// Install yt-dlp: a single static exe.
pub fn install_ytdlp(
    data_dir: &Path,
    on_progress: impl FnMut(u64, Option<u64>),
) -> Result<PathBuf, String> {
    let dest = tools_dir(data_dir).join("yt-dlp.exe");
    download(YTDLP_URL, &dest, on_progress)?;
    Ok(dest)
}

/// Install ffmpeg (+ ffprobe): download BtbN's zip, extract with Windows'
/// built-in bsdtar, keep the two exes, drop the rest.
pub fn install_ffmpeg(
    data_dir: &Path,
    on_progress: impl FnMut(u64, Option<u64>),
) -> Result<PathBuf, String> {
    let dir = tools_dir(data_dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let zip = dir.join("ffmpeg-download.zip");
    download(FFMPEG_URL, &zip, on_progress)?;

    let extract_dir = dir.join("ffmpeg-extract");
    let _ = std::fs::remove_dir_all(&extract_dir);
    std::fs::create_dir_all(&extract_dir).map_err(|e| e.to_string())?;
    // bsdtar ships with Windows 10+ and reads zip natively.
    let status = std::process::Command::new("tar")
        .args(["-xf"])
        .arg(&zip)
        .arg("-C")
        .arg(&extract_dir)
        .status()
        .map_err(|e| format!("could not run tar to extract ffmpeg: {e}"))?;
    if !status.success() {
        return Err("extracting the ffmpeg archive failed".into());
    }

    let mut installed = None;
    for name in ["ffmpeg.exe", "ffprobe.exe"] {
        let found = find_file(&extract_dir, name)
            .ok_or_else(|| format!("{name} not found in the ffmpeg archive"))?;
        let dest = dir.join(name);
        std::fs::copy(&found, &dest).map_err(|e| e.to_string())?;
        if name == "ffmpeg.exe" {
            installed = Some(dest);
        }
    }
    let _ = std::fs::remove_dir_all(&extract_dir);
    let _ = std::fs::remove_file(&zip);
    installed.ok_or_else(|| "ffmpeg.exe missing after extract".into())
}

fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if let Some(found) = find_file(&p, name) {
                return Some(found);
            }
        } else if p.file_name().is_some_and(|n| n.eq_ignore_ascii_case(name)) {
            return Some(p);
        }
    }
    None
}
