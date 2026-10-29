//! Print what exclusive-mode playback skips of the default device's Windows
//! volume (0 dB on hardware-volume devices: they apply in exclusive mode too).
//! `cargo run -p fletcher-core --example volume_probe`
fn main() {
    println!(
        "{:?} — exclusive mode bypasses {:?} dB of Windows volume",
        fletcher_core::devices::default_render_device_name(),
        fletcher_core::devices::default_render_bypassed_volume_db()
    );
}
