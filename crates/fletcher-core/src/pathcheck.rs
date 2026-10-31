//! Hearing whether Equalizer APO processes the default output (the setup
//! check's live test).
//!
//! WASAPI loopback records what a device plays after APO's post-mix stage:
//! a `Preamp: -20 dB` written to fletcher.txt reads back as exactly −20.0 dB
//! (`examples/scope_probe.rs`, docs/research/apo-scope-and-loopback.md). So a
//! faint tone played on the device and measured on its loopback shows whether
//! a change to fletcher.txt reaches the speakers, with no microphone and no
//! listening. The tone is measured with a Goertzel filter at its own
//! frequency, so music already playing barely disturbs it.

use std::collections::VecDeque;
use std::f64::consts::PI;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use wasapi::*;

/// An odd frequency, away from the usual 1 kHz test tones.
pub const TONE_HZ: f64 = 997.0;
/// −50 dBFS: barely audible at normal listening levels.
const AMPLITUDE: f64 = 0.003_162;
const RATE: u32 = 48_000;

/// Labels a WASAPI error with what Fletcher was doing.
fn ctx(what: &'static str) -> impl Fn(WasapiError) -> String {
    move |err| format!("{what}: {err}")
}

/// A faint tone playing on the default output until dropped.
pub struct Tone {
    stop: Arc<AtomicBool>,
    player: Option<std::thread::JoinHandle<()>>,
}

impl Tone {
    /// Start the tone. Fails if the default output can't be opened in shared
    /// mode (e.g. an app holds it in exclusive mode).
    pub fn start() -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let flag = stop.clone();
        let player = std::thread::spawn(move || {
            if let Err(e) = play(&flag, &ready_tx) {
                let _ = ready_tx.send(Err(e));
            }
        });
        match ready_rx.recv_timeout(Duration::from_secs(3)) {
            Ok(Ok(())) => Ok(Tone {
                stop,
                player: Some(player),
            }),
            Ok(Err(e)) => Err(e),
            Err(_) => {
                stop.store(true, Ordering::Relaxed);
                Err("the output didn't start playing".into())
            }
        }
    }
}

impl Drop for Tone {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(p) = self.player.take() {
            let _ = p.join();
        }
    }
}

fn play(
    stop: &AtomicBool,
    ready: &std::sync::mpsc::Sender<Result<(), String>>,
) -> Result<(), String> {
    let _ = initialize_mta();
    let e = ctx;
    let device = DeviceEnumerator::new()
        .map_err(e("no audio devices"))?
        .get_default_device(&Direction::Render)
        .map_err(e("no default output"))?;
    let mut client = device.get_iaudioclient().map_err(e("output"))?;
    let format = WaveFormat::new(32, 32, &SampleType::Float, RATE as usize, 2, None);
    let (period, _) = client.get_device_period().map_err(e("output"))?;
    client
        .initialize_client(
            &format,
            &Direction::Render,
            &StreamMode::EventsShared {
                autoconvert: true,
                buffer_duration_hns: period,
            },
        )
        .map_err(e("couldn't open the output"))?;
    let event = client.set_get_eventhandle().map_err(e("output"))?;
    let render = client.get_audiorenderclient().map_err(e("output"))?;
    let mut t = 0u64;
    let mut fill = |n: usize| -> Result<(), String> {
        let mut data = Vec::with_capacity(n * 8);
        for _ in 0..n {
            let s = ((t as f64 / RATE as f64) * TONE_HZ * 2.0 * PI).sin() * AMPLITUDE;
            t += 1;
            let b = (s as f32).to_le_bytes();
            data.extend_from_slice(&b);
            data.extend_from_slice(&b);
        }
        render.write_to_device(n, &data, None).map_err(e("output"))
    };
    fill(
        client
            .get_available_space_in_frames()
            .map_err(e("output"))? as usize,
    )?;
    client.start_stream().map_err(e("output"))?;
    let _ = ready.send(Ok(()));
    while !stop.load(Ordering::Relaxed) {
        if event.wait_for_event(1000).is_err() {
            break;
        }
        let space = client
            .get_available_space_in_frames()
            .map_err(e("output"))?;
        fill(space as usize)?;
    }
    let _ = client.stop_stream();
    Ok(())
}

/// The tone's level in each ear (dBFS, Goertzel power at `TONE_HZ`) over
/// `secs` of the default output's loopback.
pub fn tone_level_db(secs: f64) -> Result<(f64, f64), String> {
    let _ = initialize_mta();
    let e = ctx;
    let device = DeviceEnumerator::new()
        .map_err(e("no audio devices"))?
        .get_default_device(&Direction::Render)
        .map_err(e("no default output"))?;
    let mut client = device.get_iaudioclient().map_err(e("loopback"))?;
    let format = WaveFormat::new(32, 32, &SampleType::Float, RATE as usize, 2, None);
    client
        .initialize_client(
            &format,
            &Direction::Capture,
            &StreamMode::PollingShared {
                autoconvert: true,
                buffer_duration_hns: 1_000_000,
            },
        )
        .map_err(e("couldn't record the output"))?;
    let capture = client.get_audiocaptureclient().map_err(e("loopback"))?;
    let mut q: VecDeque<u8> = VecDeque::new();
    client.start_stream().map_err(e("loopback"))?;
    let end = Instant::now() + Duration::from_secs_f64(secs);
    while Instant::now() < end {
        std::thread::sleep(Duration::from_millis(10));
        capture
            .read_from_device_to_deque(&mut q)
            .map_err(e("loopback"))?;
    }
    let _ = client.stop_stream();
    let bytes: Vec<u8> = q.into_iter().collect();
    let ear = |ch: usize| -> Vec<f64> {
        bytes
            .as_chunks::<8>()
            .0
            .iter()
            .map(|f| f32::from_le_bytes(f[ch * 4..ch * 4 + 4].try_into().unwrap()) as f64)
            .collect()
    };
    Ok((goertzel_db(&ear(0)), goertzel_db(&ear(1))))
}

/// Power of `TONE_HZ` in `x`, as the dBFS of a sine with that amplitude's RMS.
fn goertzel_db(x: &[f64]) -> f64 {
    if x.is_empty() {
        return f64::NEG_INFINITY;
    }
    let w = 2.0 * PI * TONE_HZ / RATE as f64;
    let c = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0, 0.0);
    for &v in x {
        let s0 = v + c * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    let power = s1 * s1 + s2 * s2 - c * s1 * s2;
    // |X|² = (A·N/2)² for a sine of amplitude A; its RMS is A/√2.
    let amp = 2.0 * power.max(0.0).sqrt() / x.len() as f64;
    20.0 * (amp / std::f64::consts::SQRT_2).max(1e-12).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goertzel_reads_tone_level_through_other_sound() {
        let n = (RATE as f64 * 0.3) as usize;
        let tone = |a: f64| -> Vec<f64> {
            (0..n)
                .map(|i| (i as f64 / RATE as f64 * TONE_HZ * 2.0 * PI).sin() * a)
                .collect()
        };
        // −50 dBFS peak = −53 dBFS RMS.
        assert!((goertzel_db(&tone(AMPLITUDE)) + 53.0).abs() < 0.2);
        // A much louder 440 Hz "song" on top moves it by well under a dB.
        let mixed: Vec<f64> = tone(AMPLITUDE)
            .iter()
            .enumerate()
            .map(|(i, v)| v + 0.3 * (i as f64 / RATE as f64 * 440.0 * 2.0 * PI).sin())
            .collect();
        assert!((goertzel_db(&mixed) + 53.0).abs() < 1.0);
        assert!(goertzel_db(&vec![0.0; n]) < -200.0);
    }
}
