//! Print Equalizer APO's status on every active playback device (TB-32).
//! `cargo run -p fletcher-core --example apo_devices`

use fletcher_core::{apo, devices};

fn main() {
    match apo::detect() {
        Ok(i) => println!(
            "APO at {} — device selector: {:?}",
            i.install_path.display(),
            apo::device_selector_path(&i)
        ),
        Err(e) => println!("{e}"),
    }
    for d in devices::list_render_devices() {
        println!(
            "{:<8} {}{}",
            apo::device_apo_status(&d.id).as_str(),
            d.name,
            if d.is_default { "  (default)" } else { "" }
        );
    }
}
