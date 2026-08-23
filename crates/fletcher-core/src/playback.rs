//! WASAPI output — the track engine's device layer (ADR-0009).
//!
//! Exclusive mode is the default bypass (spike-verified: APO never touches
//! it); shared mode serves the reference calibration (the user's volume knob
//! must stay live there, Q-16) and the future config-blank fallback.
//!
//! Grown from `examples/spike_bypass.rs`. Two of its learnings are
//! load-bearing and preserved exactly: the format candidate ladder probed via
//! `is_supported_exclusive_with_quirks`, and the aligned-period retry that
//! MUST re-acquire the IAudioClient before re-initializing.
//!
//! Safety (TB-20): in exclusive mode this stream is the only volume control
//! in the system — every stream opens silent and ramps to its target gain.
//! Callers cannot skip the ramp; it is not a parameter.

use std::sync::atomic::{AtomicBool, Ordering};

use wasapi::*;

use crate::signal::Signal;

/// How the device is opened. Exclusive bypasses APO and Windows volume;
/// shared plays through the normal path (APO applies).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    Exclusive,
    Shared,
}

#[derive(Debug)]
pub enum PlaybackError {
    /// COM/device enumeration failed.
    Device(String),
    /// No candidate format was accepted in exclusive mode.
    NoFormat,
    /// Stream initialization failed (even after the aligned-period retry).
    Init(String),
    /// Failure while streaming (event timeout, write error).
    Stream(String),
}

impl std::fmt::Display for PlaybackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlaybackError::Device(e) => write!(f, "audio device error: {e}"),
            PlaybackError::NoFormat => write!(
                f,
                "the device accepted none of the candidate formats in exclusive mode"
            ),
            PlaybackError::Init(e) => write!(f, "could not open the audio stream: {e}"),
            PlaybackError::Stream(e) => write!(f, "audio stream failed: {e}"),
        }
    }
}

impl std::error::Error for PlaybackError {}

/// What the negotiation actually produced — surfaced to the UI (honesty:
/// the user can see the exact path their audio takes).
#[derive(Debug, Clone)]
pub struct StreamInfo {
    pub device: String,
    pub mode: OutputMode,
    pub bits: u16,
    pub sample_type: String,
    pub rate: u32,
}

/// An interleaved-stereo sample source, pulled on the audio thread.
/// `set_rate` is called once with the negotiated sample rate before any
/// `fill`. `fill` writes `buf.len()` samples (L R L R …); returning `false`
/// means the source is exhausted and the stream drains + stops.
pub trait Source: Send {
    fn set_rate(&mut self, fs: f64);
    fn fill(&mut self, buf: &mut [f64]) -> bool;
}

/// A generator playing for an optional duration — the calibration noise and
/// the M1 test tone. `None` = until stopped.
pub struct SignalSource {
    make: Box<dyn Fn(f64) -> Signal + Send>,
    signal: Option<Signal>,
    frames_left: Option<u64>,
    seconds: Option<f64>,
    fade_frames: u64,
}

impl SignalSource {
    pub fn new(make: impl Fn(f64) -> Signal + Send + 'static, seconds: Option<f64>) -> Self {
        SignalSource {
            make: Box::new(make),
            signal: None,
            frames_left: None,
            seconds,
            fade_frames: 0,
        }
    }
}

impl Source for SignalSource {
    fn set_rate(&mut self, fs: f64) {
        self.signal = Some((self.make)(fs));
        self.frames_left = self.seconds.map(|s| (s * fs) as u64);
        self.fade_frames = (0.3 * fs) as u64;
    }

    fn fill(&mut self, buf: &mut [f64]) -> bool {
        let Some(sig) = self.signal.as_mut() else {
            return false;
        };
        for frame in buf.chunks_exact_mut(2) {
            let (done, env) = match self.frames_left.as_mut() {
                Some(0) => (true, 0.0),
                Some(n) => {
                    *n -= 1;
                    // Timed signals fade out over their last 300 ms — nothing
                    // this engine plays ever stops sharply.
                    let env = if *n < self.fade_frames {
                        0.5 - 0.5
                            * (std::f64::consts::PI * *n as f64 / self.fade_frames as f64).cos()
                    } else {
                        1.0
                    };
                    (false, env)
                }
                None => (false, 1.0),
            };
            if done {
                frame[0] = 0.0;
                frame[1] = 0.0;
                continue;
            }
            let s = sig.next_sample() * env;
            frame[0] = s;
            frame[1] = s;
        }
        self.frames_left != Some(0)
    }
}

/// Open the default render device in `mode`, ramp in, and pull `source`
/// until it is exhausted or `stop` is raised. Blocking — call it on a thread
/// that owns the stream for its whole life. `on_start` fires once with the
/// negotiated format, after the stream is live.
pub fn play(
    mode: OutputMode,
    source: &mut dyn Source,
    target_gain: f64,
    stop: &AtomicBool,
    mut on_start: impl FnMut(&StreamInfo),
) -> Result<(), PlaybackError> {
    // Per-thread COM init; "already initialized" is fine.
    let _ = initialize_mta();

    let dev = |e: String| PlaybackError::Device(e);
    let enumerator = DeviceEnumerator::new().map_err(|e| dev(e.to_string()))?;
    let device = enumerator
        .get_default_device(&Direction::Render)
        .map_err(|e| dev(e.to_string()))?;
    let device_name = device.get_friendlyname().unwrap_or_else(|_| "?".into());
    let mut audio_client = device.get_iaudioclient().map_err(|e| dev(e.to_string()))?;

    let (format, stream_mode) = match mode {
        OutputMode::Exclusive => {
            // The spike's ladder, in preference order.
            let candidates = [
                WaveFormat::new(32, 32, &SampleType::Float, 48000, 2, None),
                WaveFormat::new(24, 24, &SampleType::Int, 48000, 2, None),
                WaveFormat::new(16, 16, &SampleType::Int, 48000, 2, None),
                WaveFormat::new(16, 16, &SampleType::Int, 44100, 2, None),
            ];
            let format = candidates
                .iter()
                .find(|f| audio_client.is_supported_exclusive_with_quirks(f).is_ok())
                .ok_or(PlaybackError::NoFormat)?
                .clone();
            let (def_period, _) = audio_client
                .get_device_period()
                .map_err(|e| dev(e.to_string()))?;
            (
                format,
                StreamMode::EventsExclusive {
                    period_hns: def_period,
                },
            )
        }
        OutputMode::Shared => {
            let format = WaveFormat::new(32, 32, &SampleType::Float, 48000, 2, None);
            let needs_convert = !matches!(
                audio_client.is_supported(&format, &ShareMode::Shared),
                Ok(None)
            );
            let (def_period, _) = audio_client
                .get_device_period()
                .map_err(|e| dev(e.to_string()))?;
            (
                format,
                StreamMode::EventsShared {
                    autoconvert: needs_convert,
                    buffer_duration_hns: def_period,
                },
            )
        }
    };

    if let Err(e) = audio_client.initialize_client(&format, &Direction::Render, &stream_mode) {
        if mode == OutputMode::Exclusive {
            // Unaligned-buffer failure: retry once with an aligned period.
            // The client re-acquisition is load-bearing — a failed
            // IAudioClient cannot be re-initialized.
            let (_, min_period) = audio_client
                .get_device_period()
                .map_err(|e2| dev(e2.to_string()))?;
            let aligned = audio_client
                .calculate_aligned_period_near(3 * min_period / 2, Some(128), &format)
                .map_err(|e2| PlaybackError::Init(format!("{e} / align: {e2}")))?;
            audio_client = device
                .get_iaudioclient()
                .map_err(|e2| dev(e2.to_string()))?;
            audio_client
                .initialize_client(
                    &format,
                    &Direction::Render,
                    &StreamMode::EventsExclusive {
                        period_hns: aligned,
                    },
                )
                .map_err(|e2| PlaybackError::Init(format!("{e} / retry: {e2}")))?;
        } else {
            return Err(PlaybackError::Init(e.to_string()));
        }
    }

    let h_event = audio_client
        .set_get_eventhandle()
        .map_err(|e| PlaybackError::Init(e.to_string()))?;
    let render_client = audio_client
        .get_audiorenderclient()
        .map_err(|e| PlaybackError::Init(e.to_string()))?;

    let blockalign = format.get_blockalign() as usize;
    let bytes_per_sample = blockalign / 2;
    let rate = format.get_samplespersec();
    let is_float = format.get_bitspersample() == 32;
    source.set_rate(rate as f64);

    let info = StreamInfo {
        device: device_name,
        mode,
        bits: format.get_bitspersample(),
        sample_type: if is_float {
            "float".into()
        } else {
            "int".into()
        },
        rate,
    };

    // TB-20: open silent, raised-cosine ramp to target over ~300 ms. A stop
    // request fades out over ~150 ms the same way — the engine never cuts.
    let ramp_frames = (0.3 * rate as f64) as u64;
    let fade_frames = (0.15 * rate as f64) as u64;
    let mut frames_out: u64 = 0;
    let mut fbuf: Vec<f64> = Vec::new();
    let mut data: Vec<u8> = Vec::new();

    let mut write = |n: usize,
                     source: &mut dyn Source,
                     frames_out: &mut u64,
                     fade_start: Option<u64>|
     -> Result<bool, PlaybackError> {
        {
            fbuf.resize(n * 2, 0.0);
            let more = source.fill(&mut fbuf);
            data.resize(n * blockalign, 0);
            for (i, frame) in data.chunks_exact_mut(blockalign).enumerate() {
                let mut ramp = if *frames_out < ramp_frames {
                    let x = *frames_out as f64 / ramp_frames as f64;
                    0.5 - 0.5 * (std::f64::consts::PI * x).cos()
                } else {
                    1.0
                };
                if let Some(fs0) = fade_start {
                    let k = (frames_out.saturating_sub(fs0)) as f64 / fade_frames as f64;
                    ramp *= 0.5 + 0.5 * (std::f64::consts::PI * k.min(1.0)).cos();
                }
                *frames_out += 1;
                let g = target_gain * ramp;
                for (ch, sample) in frame
                    .chunks_exact_mut(bytes_per_sample)
                    .zip([fbuf[i * 2] * g, fbuf[i * 2 + 1] * g])
                {
                    let s = sample.clamp(-1.0, 1.0);
                    if is_float {
                        ch.copy_from_slice(&(s as f32).to_le_bytes());
                    } else if bytes_per_sample == 2 {
                        ch.copy_from_slice(&((s * 32767.0) as i16).to_le_bytes());
                    } else {
                        // Packed 24-bit: low bytes of an i32, little-endian.
                        let v = (s * 8388607.0) as i32;
                        ch.copy_from_slice(&v.to_le_bytes()[..bytes_per_sample]);
                    }
                }
            }
            render_client
                .write_to_device(n, &data, None)
                .map_err(|e| PlaybackError::Stream(e.to_string()))?;
            Ok(more)
        }
    };

    // Prime before start (the spike's ordering).
    let space = audio_client
        .get_available_space_in_frames()
        .map_err(|e| PlaybackError::Stream(e.to_string()))?;
    let mut more = write(space as usize, source, &mut frames_out, None)?;
    audio_client
        .start_stream()
        .map_err(|e| PlaybackError::Stream(e.to_string()))?;
    on_start(&info);

    let mut fade_start: Option<u64> = None;
    let result = loop {
        if stop.load(Ordering::Relaxed) && fade_start.is_none() {
            fade_start = Some(frames_out); // begin the stop fade, keep streaming
        }
        if let Some(fs0) = fade_start
            && frames_out >= fs0 + fade_frames
        {
            // Fade fully written; let it drain before releasing the device.
            let _ = h_event.wait_for_event(200);
            break Ok(());
        }
        if !more {
            // Let the last buffer drain before releasing the device.
            let _ = h_event.wait_for_event(200);
            break Ok(());
        }
        if h_event.wait_for_event(1000).is_err() {
            break Err(PlaybackError::Stream("event timeout".into()));
        }
        let space = match audio_client.get_available_space_in_frames() {
            Ok(s) => s,
            Err(e) => break Err(PlaybackError::Stream(e.to_string())),
        };
        more = match write(space as usize, source, &mut frames_out, fade_start) {
            Ok(m) => m,
            Err(e) => break Err(e),
        };
    };

    let _ = audio_client.stop_stream();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::{DEFAULT_AMP, SignalKind};

    /// Requires a real render endpoint — dev machine only (apo.rs pattern).
    #[test]
    #[ignore = "requires a local audio endpoint"]
    fn exclusive_tone_opens_ramps_and_releases() {
        let stop = AtomicBool::new(false);
        let mut src = SignalSource::new(
            |fs| Signal::new(SignalKind::Sine { hz: 440.0 }, DEFAULT_AMP, fs, 1),
            Some(1.0),
        );
        let mut started = None;
        play(OutputMode::Exclusive, &mut src, 1.0, &stop, |info| {
            started = Some(info.clone())
        })
        .expect("exclusive playback");
        let info = started.expect("on_start fired");
        assert!(info.rate == 48000 || info.rate == 44100);
    }
}
