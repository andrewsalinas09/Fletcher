//! Filesystem helpers with the safety properties config writes need.

use std::io::Write;
use std::path::Path;

/// Atomic-ish write: contents land in a sibling temp file, then rename over
/// the target (std::fs::rename replaces on Windows). A crash mid-write leaves
/// the original untouched (TB-11: never a half-written config for APO's
/// watcher to pick up).
///
/// The rename retries briefly: on Windows it fails with a sharing violation /
/// access-denied while another process (Equalizer APO reloading, an indexer)
/// momentarily holds the destination open — routine under rapid live edits.
pub fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("fletcher-tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(contents.as_bytes())?;
        f.sync_all()?;
    }
    let mut delay = std::time::Duration::from_millis(8);
    for attempt in 0..6 {
        match std::fs::rename(&tmp, path) {
            Ok(()) => return Ok(()),
            Err(e) => {
                let retryable = e.kind() == std::io::ErrorKind::PermissionDenied
                    || matches!(e.raw_os_error(), Some(5) | Some(32) | Some(33));
                if !retryable || attempt == 5 {
                    return Err(e);
                }
            }
        }
        std::thread::sleep(delay);
        delay *= 2;
    }
    unreachable!()
}
