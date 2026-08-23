//! Audio device enumeration (MMDevice via the wasapi crate).

use std::sync::Once;

fn ensure_com() {
    static COM: Once = Once::new();
    COM.call_once(|| {
        // Per-process MTA init; harmless if the thread is already initialized.
        let _ = wasapi::initialize_mta();
    });
}

/// Friendly name of the default render (playback) device, if resolvable.
pub fn default_render_device_name() -> Option<String> {
    ensure_com();
    let enumerator = wasapi::DeviceEnumerator::new().ok()?;
    let device = enumerator
        .get_default_device(&wasapi::Direction::Render)
        .ok()?;
    device.get_friendlyname().ok()
}
