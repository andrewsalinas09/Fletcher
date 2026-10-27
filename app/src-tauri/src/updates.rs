//! Self-update from this repository's GitHub releases (ADR-0015; config in
//! tauri.conf.json → plugins.updater).
//!
//! Check and install run here, not from the webview, so the safety rules can't
//! be skipped: installing closes the app (the NSIS installer replaces it and
//! reopens it), so it is refused while a blind trial or hearing-profile session
//! is in progress — closing would throw the session away — and any playing
//! audio is faded out first, never cut (TB-33). Every download is verified
//! against the embedded public key by the updater plugin before it runs.
use serde::Serialize;
use tauri::Emitter;
use tauri_plugin_updater::UpdaterExt;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Available {
    version: String,
    notes: Option<String>,
    date: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    current: String,
    available: Option<Available>,
    /// Dev builds run from target/; an installer can't replace them.
    can_install: bool,
}

/// Why installing must wait, if it must.
fn busy() -> Option<&'static str> {
    if crate::TRIAL.lock().unwrap().is_some() {
        Some("A blind test is running. Finish or cancel it, then install.")
    } else if crate::profile_running() {
        Some("A hearing-profile session is running. Finish or pause it, then install.")
    } else {
        None
    }
}

#[tauri::command]
pub async fn update_check(app: tauri::AppHandle) -> Result<UpdateStatus, String> {
    let current = app.package_info().version.to_string();
    let found = match app.updater().map_err(|e| e.to_string())?.check().await {
        Ok(found) => found,
        // The latest release has no build for this platform: nothing to
        // install, not a failure.
        Err(
            tauri_plugin_updater::Error::TargetNotFound(_)
            | tauri_plugin_updater::Error::TargetsNotFound(_),
        ) => None,
        Err(e) => return Err(format!("Couldn't reach the update server: {e}")),
    };
    let available = found.map(|update| Available {
        version: update.version.clone(),
        notes: update.body.clone().filter(|notes| !notes.trim().is_empty()),
        date: update.date.map(|date| date.date().to_string()),
    });
    Ok(UpdateStatus {
        current,
        available,
        can_install: !cfg!(debug_assertions),
    })
}

#[tauri::command]
pub async fn update_install(app: tauri::AppHandle) -> Result<(), String> {
    if cfg!(debug_assertions) {
        return Err(
            "This is a development build. Updates install only in the released app.".into(),
        );
    }
    if let Some(reason) = busy() {
        return Err(reason.into());
    }
    let update = app
        .updater()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| format!("Couldn't reach the update server: {e}"))?
        .ok_or("This version is already up to date.")?;
    let progress = app.clone();
    let mut received = 0u64;
    let bytes = update
        .download(
            move |chunk, total| {
                received += chunk as u64;
                let _ = progress.emit("update-progress", (received, total));
            },
            || {},
        )
        .await
        .map_err(|e| format!("The update could not be downloaded: {e}"))?;
    // A session may have started while downloading; this is the last safe moment.
    if let Some(reason) = busy() {
        return Err(reason.into());
    }
    // Fade out anything playing and wait for the device to be released, so the
    // installer closing the app never cuts a stream mid-buffer.
    crate::stop_cal_noise();
    if let Some(done) = crate::track_stop_inner() {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !done.load(std::sync::atomic::Ordering::Relaxed)
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    // Windows: the installer closes the app, replaces it and reopens it; at
    // launch `apply_side` rewrites fletcher.txt deterministically, so nothing
    // transient (a history preview) survives the restart.
    update
        .install(bytes)
        .map_err(|e| format!("The update could not be installed: {e}"))
}
