//! Filesystem helpers with the safety properties config writes need.

use std::io::Write;
use std::path::Path;

/// Atomic-ish write: contents land in a sibling temp file, then rename over
/// the target (std::fs::rename replaces on Windows). A crash mid-write leaves
/// the original untouched (TB-11: never a half-written config for APO's
/// watcher to pick up).
pub fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("fletcher-tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(contents.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}
