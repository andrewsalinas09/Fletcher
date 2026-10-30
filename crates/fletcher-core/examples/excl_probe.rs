//! Silent exclusive-mode probe (issue #9): for each render device whose name
//! contains the argument (default: every device), ask IsFormatSupported for
//! each exclusive candidate, then try Initialize at the default, minimum and
//! 128-byte-aligned periods. Never calls Start, so nothing plays.
//!
//!   cargo run -p fletcher-core --example excl_probe -- "WH-1000XM6"

use wasapi::{DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat, initialize_mta};

fn main() {
    let filter = std::env::args().nth(1).unwrap_or_default();
    let _ = initialize_mta();
    let enumerator = DeviceEnumerator::new().expect("enumerator");
    let collection = enumerator
        .get_device_collection(&Direction::Render)
        .expect("collection");
    let candidates = [
        (
            "f32/48k",
            WaveFormat::new(32, 32, &SampleType::Float, 48000, 2, None),
        ),
        (
            "i24/48k",
            WaveFormat::new(24, 24, &SampleType::Int, 48000, 2, None),
        ),
        (
            "i16/48k",
            WaveFormat::new(16, 16, &SampleType::Int, 48000, 2, None),
        ),
        (
            "i16/44.1k",
            WaveFormat::new(16, 16, &SampleType::Int, 44100, 2, None),
        ),
    ];
    for dev in &collection {
        let Ok(dev) = dev else { continue };
        let name = dev.get_friendlyname().unwrap_or_default();
        if !name.contains(&filter) {
            continue;
        }
        println!("== {name}");
        let Ok(client) = dev.get_iaudioclient() else {
            println!("   no audio client");
            continue;
        };
        if let Ok(mix) = client.get_mixformat() {
            println!(
                "   mix: {} bit {:?} {} Hz {} ch",
                mix.get_bitspersample(),
                mix.get_subformat(),
                mix.get_samplespersec(),
                mix.get_nchannels()
            );
        }
        let (def, min) = client.get_device_period().unwrap_or((0, 0));
        println!("   period: default {} µs, min {} µs", def / 10, min / 10);
        for (label, fmt) in &candidates {
            let supported = client.is_supported_exclusive_with_quirks(fmt);
            print!(
                "   {label:9} IsFormatSupported: {:<4}",
                if supported.is_ok() { "OK" } else { "no" }
            );
            if supported.is_err() {
                println!();
                continue;
            }
            let mut periods = vec![("default", def), ("min", min)];
            if let Ok(c) = dev.get_iaudioclient()
                && let Ok(a) = c.calculate_aligned_period_near(3 * min / 2, Some(128), fmt)
            {
                periods.push(("aligned", a));
            }
            for (pl, p) in periods {
                // A failed client can't be re-initialized: a fresh one per try.
                let result = dev
                    .get_iaudioclient()
                    .map_err(|e| e.to_string())
                    .and_then(|mut c| {
                        c.initialize_client(
                            fmt,
                            &Direction::Render,
                            &StreamMode::EventsExclusive { period_hns: p },
                        )
                        .map_err(|e| e.to_string())
                    });
                print!(
                    " | Init {pl}: {}",
                    match result {
                        Ok(()) => "OK".to_string(),
                        Err(e) => e.chars().filter(|c| !c.is_control()).take(60).collect(),
                    }
                );
            }
            println!();
        }
    }
}
