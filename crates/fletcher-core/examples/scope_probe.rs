//! Probe: which APO selections (`Device:`, `Stage:`, `Channel:`) made inside an
//! included file still apply after the include returns, and does WASAPI
//! loopback hear APO's post-mix output at all?
//!
//! Plays a quiet 1 kHz tone (-30 dBFS) on the default output, records the same
//! device through loopback, and for each scenario rewrites fletcher.txt,
//! probe-inc.txt and peace.txt, waits for APO to reload, and reports the level
//! change per ear against the baseline. Every scenario adds `Preamp: -20 dB`
//! somewhere; "-20" means it applied, "0" means it was skipped.
//!
//! Restores config.txt, peace.txt and fletcher.txt byte-for-byte on exit,
//! including after a panic. Run: `cargo run --example scope_probe`.

use std::collections::VecDeque;
use std::f64::consts::PI;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use wasapi::*;

const AMPLITUDE: f64 = 0.0316; // -30 dBFS
const TONE_HZ: f64 = 1000.0;

struct Restore {
    dir: PathBuf,
    saved: Vec<(&'static str, Option<Vec<u8>>)>,
}

impl Restore {
    fn new(dir: PathBuf) -> Self {
        let saved = ["config.txt", "peace.txt", "fletcher.txt", "probe-inc.txt"]
            .into_iter()
            .map(|n| (n, std::fs::read(dir.join(n)).ok()))
            .collect();
        Restore { dir, saved }
    }
}

impl Drop for Restore {
    fn drop(&mut self) {
        for (name, bytes) in &self.saved {
            let p = self.dir.join(name);
            match bytes {
                Some(b) => std::fs::write(&p, b).expect("restore"),
                None => {
                    let _ = std::fs::remove_file(&p);
                }
            }
        }
        println!("restored config.txt, peace.txt, fletcher.txt");
    }
}

fn play(stop: Arc<AtomicBool>) {
    initialize_mta().ok().unwrap();
    let device = DeviceEnumerator::new()
        .unwrap()
        .get_default_device(&Direction::Render)
        .unwrap();
    let mut client = device.get_iaudioclient().unwrap();
    let format = WaveFormat::new(32, 32, &SampleType::Float, 48000, 2, None);
    let (period, _) = client.get_device_period().unwrap();
    client
        .initialize_client(
            &format,
            &Direction::Render,
            &StreamMode::EventsShared {
                autoconvert: true,
                buffer_duration_hns: period,
            },
        )
        .unwrap();
    let event = client.set_get_eventhandle().unwrap();
    let render = client.get_audiorenderclient().unwrap();
    let mut t = 0u64;
    let mut fill = |n: usize| {
        let mut data = Vec::with_capacity(n * 8);
        for _ in 0..n {
            let s = ((t as f64 / 48000.0) * TONE_HZ * 2.0 * PI).sin() * AMPLITUDE;
            t += 1;
            let b = (s as f32).to_le_bytes();
            data.extend_from_slice(&b);
            data.extend_from_slice(&b);
        }
        render.write_to_device(n, &data, None).unwrap();
    };
    fill(client.get_available_space_in_frames().unwrap() as usize);
    client.start_stream().unwrap();
    while !stop.load(Ordering::Relaxed) {
        if event.wait_for_event(1000).is_err() {
            break;
        }
        fill(client.get_available_space_in_frames().unwrap() as usize);
    }
    client.stop_stream().unwrap();
}

/// RMS in dBFS of each ear over `secs` of loopback.
fn measure(secs: f64) -> (f64, f64) {
    let device = DeviceEnumerator::new()
        .unwrap()
        .get_default_device(&Direction::Render)
        .unwrap();
    let mut client = device.get_iaudioclient().unwrap();
    let format = WaveFormat::new(32, 32, &SampleType::Float, 48000, 2, None);
    client
        .initialize_client(
            &format,
            &Direction::Capture,
            &StreamMode::PollingShared {
                autoconvert: true,
                buffer_duration_hns: 1_000_000,
            },
        )
        .unwrap();
    let capture = client.get_audiocaptureclient().unwrap();
    let mut q: VecDeque<u8> = VecDeque::new();
    client.start_stream().unwrap();
    let end = Instant::now() + Duration::from_secs_f64(secs);
    while Instant::now() < end {
        std::thread::sleep(Duration::from_millis(10));
        let _ = capture.read_from_device_to_deque(&mut q);
    }
    client.stop_stream().unwrap();
    let bytes: Vec<u8> = q.into_iter().collect();
    let (mut l, mut r, mut n) = (0.0f64, 0.0f64, 0usize);
    for f in bytes.as_chunks::<8>().0 {
        let a = f32::from_le_bytes(f[0..4].try_into().unwrap()) as f64;
        let b = f32::from_le_bytes(f[4..8].try_into().unwrap()) as f64;
        l += a * a;
        r += b * b;
        n += 1;
    }
    let db = |s: f64| 10.0 * (s / n.max(1) as f64).max(1e-20).log10();
    (db(l), db(r))
}

fn main() {
    let dir = fletcher_core::apo::detect().unwrap().config_path;
    initialize_mta().ok().unwrap();
    println!(
        "device: {}",
        fletcher_core::devices::default_render_device_name().unwrap_or_default()
    );
    let guard = Restore::new(dir.clone());
    let config = String::from_utf8(guard.saved[0].1.clone().unwrap_or_default()).unwrap();
    println!("config.txt:\n{config}---");

    let stop = Arc::new(AtomicBool::new(false));
    let player = {
        let stop = stop.clone();
        std::thread::spawn(move || play(stop))
    };
    std::thread::sleep(Duration::from_millis(800));

    let set = |name: &str, text: &str| std::fs::write(dir.join(name), text).unwrap();
    let run = || {
        std::thread::sleep(Duration::from_millis(2500)); // APO reload
        measure(1.0)
    };

    // (label, fletcher.txt, probe-inc.txt, peace.txt)
    let scenarios: [(&str, &str, &str, &str); 8] = [
        ("baseline", "", "", ""),
        ("1 Preamp -20 in fletcher.txt", "Preamp: -20 dB\n", "", ""),
        (
            "2 Device: nomatch, then Preamp (same file)",
            "Device: NoSuchDeviceXYZ\nPreamp: -20 dB\n",
            "",
            "",
        ),
        (
            "3 Device: nomatch inside include, Preamp after it",
            "Include: probe-inc.txt\nPreamp: -20 dB\n",
            "Device: NoSuchDeviceXYZ\n",
            "",
        ),
        (
            "4 Stage: capture inside include, Preamp after it",
            "Include: probe-inc.txt\nPreamp: -20 dB\n",
            "Stage: capture\n",
            "",
        ),
        (
            "5 Channel: L inside include, Preamp after it",
            "Include: probe-inc.txt\nPreamp: -20 dB\n",
            "Channel: L\n",
            "",
        ),
        (
            "6 peace.txt ends on Device: nomatch, Preamp in fletcher.txt",
            "Preamp: -20 dB\n",
            "",
            "Device: all\nPreamp: 0 dB\nDevice: NoSuchDeviceXYZ\nPreamp: 0 dB\n",
        ),
        (
            "7 peace.txt ends on Stage: pre-mix, Preamp in fletcher.txt",
            "Preamp: -20 dB\n",
            "",
            "Stage: pre-mix\n",
        ),
    ];
    let mut base = (0.0, 0.0);
    for (label, fl, inc, peace) in scenarios {
        if label != "baseline" {
            set("fletcher.txt", "");
            set("probe-inc.txt", "");
            set("peace.txt", "");
            let (l, r) = run();
            println!(
                "{:62} L {:+6.1} dB  R {:+6.1} dB",
                "  (back to baseline)",
                l - base.0,
                r - base.1
            );
        }
        set("fletcher.txt", fl);
        set("probe-inc.txt", inc);
        set("peace.txt", peace);
        let (l, r) = run();
        for (name, want) in [("fletcher.txt", fl), ("peace.txt", peace)] {
            let got = std::fs::read_to_string(dir.join(name)).unwrap_or_default();
            if got != want {
                println!("  !! {name} was rewritten by someone else:\n{got}");
            }
        }
        if label == "baseline" {
            base = (l, r);
            println!("{label:62} L {l:7.1} dBFS  R {r:7.1} dBFS");
        } else {
            println!(
                "{label:62} L {:+6.1} dB  R {:+6.1} dB",
                l - base.0,
                r - base.1
            );
        }
    }
    stop.store(true, Ordering::Relaxed);
    player.join().unwrap();
    drop(guard);
}
