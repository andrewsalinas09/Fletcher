//! Q-04 spike: does WASAPI exclusive mode bypass Equalizer APO?
//!
//! Protocol: activate a dramatic APO filter (e.g. `Filter: ON LP Fc 500 Hz`
//! in fletcher.txt), then play a 3 kHz tone both ways:
//!   cargo run --bin spike_bypass -- shared     -> tone should be muffled (APO applies)
//!   cargo run --bin spike_bypass -- exclusive  -> hypothesis: tone clear (APO bypassed)
//!
//! Tone is deliberately quiet (-24 dBFS): exclusive mode bypasses Windows
//! volume, so raw DAC level reaches the amp (TB-20).

use std::f64::consts::PI;
use wasapi::*;

const TONE_HZ: f64 = 3000.0;
const AMPLITUDE: f64 = 0.06; // ~ -24 dBFS
const SECONDS: f64 = 4.0;

struct Sine {
    time: f64,
    delta_t: f64,
}

impl Sine {
    fn next(&mut self) -> f64 {
        self.time += self.delta_t;
        (TONE_HZ * self.time * PI * 2.0).sin() * AMPLITUDE
    }
}

fn main() {
    let mode_arg = std::env::args().nth(1).unwrap_or_default();
    match mode_arg.as_str() {
        "shared" | "exclusive" => {}
        _ => {
            eprintln!("usage: spike_bypass <shared|exclusive>");
            std::process::exit(2);
        }
    }
    let exclusive = mode_arg == "exclusive";

    initialize_mta().ok().unwrap();
    let enumerator = DeviceEnumerator::new().unwrap();
    let device = enumerator.get_default_device(&Direction::Render).unwrap();
    println!(
        "Device: {}",
        device.get_friendlyname().unwrap_or_else(|_| "?".into())
    );
    let mut audio_client = device.get_iaudioclient().unwrap();

    let (format, stream_mode) = if exclusive {
        // Find an exclusive-capable format among common candidates.
        let candidates = [
            WaveFormat::new(32, 32, &SampleType::Float, 48000, 2, None),
            WaveFormat::new(24, 24, &SampleType::Int, 48000, 2, None),
            WaveFormat::new(16, 16, &SampleType::Int, 48000, 2, None),
            WaveFormat::new(16, 16, &SampleType::Int, 44100, 2, None),
        ];
        let format = candidates
            .iter()
            .find(|f| audio_client.is_supported_exclusive_with_quirks(f).is_ok())
            .expect("no candidate format supported in exclusive mode")
            .clone();
        println!(
            "Exclusive format: {} bit {:?} @ {} Hz",
            format.get_bitspersample(),
            format.get_subformat().unwrap(),
            format.get_samplespersec()
        );
        let (def_period, _min_period) = audio_client.get_device_period().unwrap();
        (
            format,
            StreamMode::EventsExclusive {
                period_hns: def_period,
            },
        )
    } else {
        let format = WaveFormat::new(32, 32, &SampleType::Float, 48000, 2, None);
        let needs_convert = !matches!(
            audio_client.is_supported(&format, &ShareMode::Shared),
            Ok(None)
        );
        let (def_period, _) = audio_client.get_device_period().unwrap();
        (
            format,
            StreamMode::EventsShared {
                autoconvert: needs_convert,
                buffer_duration_hns: def_period,
            },
        )
    };

    // Exclusive init can fail on unaligned buffers; retry once with an aligned period.
    if let Err(e) = audio_client.initialize_client(&format, &Direction::Render, &stream_mode) {
        if exclusive {
            println!("init failed ({e}), retrying with 128-byte-aligned period");
            let (_, min_period) = audio_client.get_device_period().unwrap();
            let aligned = audio_client
                .calculate_aligned_period_near(3 * min_period / 2, Some(128), &format)
                .unwrap();
            audio_client = device.get_iaudioclient().unwrap();
            audio_client
                .initialize_client(
                    &format,
                    &Direction::Render,
                    &StreamMode::EventsExclusive {
                        period_hns: aligned,
                    },
                )
                .unwrap();
        } else {
            panic!("shared init failed: {e}");
        }
    }

    let h_event = audio_client.set_get_eventhandle().unwrap();
    let render_client = audio_client.get_audiorenderclient().unwrap();
    let blockalign = format.get_blockalign() as usize;
    let bytes_per_sample = blockalign / 2;
    let sample_rate = format.get_samplespersec() as f64;
    let is_float = format.get_bitspersample() == 32;
    let mut tone = Sine {
        time: 0.0,
        delta_t: 1.0 / sample_rate,
    };

    let write_frames = |n: usize, tone: &mut Sine| {
        let mut data = vec![0u8; n * blockalign];
        for frame in data.chunks_exact_mut(blockalign) {
            let s = tone.next();
            let bytes: Vec<u8> = if is_float {
                (s as f32).to_le_bytes().to_vec()
            } else {
                // 16- or 24-bit signed int, little-endian, low bytes of i32
                let v = (s
                    * 8388607.0
                    * if bytes_per_sample == 2 {
                        1.0 / 256.0
                    } else {
                        1.0
                    }) as i32;
                v.to_le_bytes()[..bytes_per_sample].to_vec()
            };
            for ch in frame.chunks_exact_mut(bytes_per_sample) {
                ch.copy_from_slice(&bytes);
            }
        }
        render_client.write_to_device(n, &data, None).unwrap();
    };

    let space = audio_client.get_available_space_in_frames().unwrap();
    write_frames(space as usize, &mut tone);
    audio_client.start_stream().unwrap();
    println!("Playing {TONE_HZ} Hz tone for {SECONDS} s in {mode_arg} mode...");

    let total_frames = (SECONDS * sample_rate) as u64;
    let mut written: u64 = space as u64;
    while written < total_frames {
        if h_event.wait_for_event(1000).is_err() {
            eprintln!("event timeout, stopping");
            break;
        }
        let space = audio_client.get_available_space_in_frames().unwrap();
        write_frames(space as usize, &mut tone);
        written += space as u64;
    }
    audio_client.stop_stream().unwrap();
    println!("Done.");
}
