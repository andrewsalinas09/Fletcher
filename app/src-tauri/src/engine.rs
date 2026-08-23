//! The track engine's session layer: decode via ffmpeg into a PCM cache,
//! play it through fletcher_core::playback on a dedicated thread, and expose
//! transport state to the UI over events.
//!
//! One session at a time (the device is a singleton in exclusive mode); the
//! `TrackShared` atomics are the realtime-safe control surface — the audio
//! thread only ever reads them.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use fletcher_core::dsp::{ChainProcessor, FilterSpec};
use fletcher_core::playback::{self, OutputMode, Source};

/// Both buses' configuration — swapped in whole on live edits.
#[derive(Clone, Default)]
pub struct BusConfig {
    /// Bus A: the user's chain (enabled filters only) + its matched preamp.
    pub specs: Vec<FilterSpec>,
    pub preamp_a_db: f64,
    /// Bus B: flat at its matched preamp, plus the true-LUFS trim measured
    /// against bus A on this exact track (ADR-0003: track mode measures).
    pub preamp_b_db: f64,
    pub trim_b_db: f64,
    /// Honesty switch snapshot: false = no trim, comparisons marked unmatched.
    pub matched: bool,
    /// Exclusive sessions run the EQ in-engine. Shared fallback must NOT —
    /// APO still processes the engine's output there (TB-03 double-processing),
    /// so A/B stays on the config-swap path and the engine plays raw.
    pub in_engine_eq: bool,
}

/// Realtime-safe transport + bus state shared between commands and the audio
/// thread. The Mutex is swapped rarely (edits, side data) and held briefly.
pub struct TrackShared {
    /// Playhead in frames at the session's sample rate.
    pub pos: AtomicU64,
    pub paused: AtomicBool,
    /// false = A (the chain), true = B (flat).
    pub side_b: AtomicBool,
    /// Bumped on every BusConfig swap; the audio thread rebuilds on change.
    pub chain_gen: AtomicU64,
    pub bus: Mutex<BusConfig>,
    pub total_frames: u64,
    pub rate: u32,
}

/// The audio-thread source: pulls interleaved stereo f32 PCM from memory and
/// runs BOTH buses every frame — the bypass bus goes through identical
/// buffering so switching is symmetric in latency and audibility (TB-12).
/// The equal-power crossfade (~15 ms) makes the switch seamless. Pause plays
/// silence (the stream stays open — resume is instant); end of track ends
/// the session.
pub struct TrackSource {
    pcm: Arc<Vec<f32>>,
    shared: Arc<TrackShared>,
    seen_gen: u64,
    bus_a: ChainProcessor,
    bus_b: ChainProcessor,
    gain_b_extra: f64,
    in_engine_eq: bool,
    /// Crossfade position 0 (A) .. 1 (B); slews toward the target side.
    xfade: f64,
    xstep: f64,
}

impl TrackSource {
    pub fn new(pcm: Arc<Vec<f32>>, shared: Arc<TrackShared>) -> Self {
        TrackSource {
            pcm,
            shared,
            seen_gen: u64::MAX,
            bus_a: ChainProcessor::new(&[], 0.0, 48000.0),
            bus_b: ChainProcessor::new(&[], 0.0, 48000.0),
            gain_b_extra: 1.0,
            in_engine_eq: false,
            xfade: 0.0,
            xstep: 1.0 / (0.015 * 48000.0),
        }
    }

    fn refresh_buses(&mut self, fs: f64) {
        let gen = self.shared.chain_gen.load(Ordering::Relaxed);
        if gen == self.seen_gen {
            return;
        }
        self.seen_gen = gen;
        let cfg = self.shared.bus.lock().unwrap().clone();
        self.in_engine_eq = cfg.in_engine_eq;
        self.bus_a.set_chain(&cfg.specs, cfg.preamp_a_db, fs);
        self.bus_b.set_chain(&[], cfg.preamp_b_db, fs);
        self.gain_b_extra = if cfg.matched {
            10f64.powf(cfg.trim_b_db / 20.0)
        } else {
            1.0
        };
    }
}

impl Source for TrackSource {
    fn set_rate(&mut self, fs: f64) {
        // PCM is pre-decoded at the probed rate (playback::probe_rate).
        self.xstep = 1.0 / (0.015 * fs);
        self.xfade = if self.shared.side_b.load(Ordering::Relaxed) {
            1.0
        } else {
            0.0
        };
        self.refresh_buses(fs);
    }

    fn fill(&mut self, buf: &mut [f64]) -> bool {
        let fs = self.shared.rate as f64;
        self.refresh_buses(fs);
        if self.shared.paused.load(Ordering::Relaxed) {
            buf.fill(0.0);
            return true;
        }
        let mut pos = self.shared.pos.load(Ordering::Relaxed) as usize;
        let total = self.shared.total_frames as usize;
        let target = if self.shared.side_b.load(Ordering::Relaxed) {
            1.0
        } else {
            0.0
        };
        for frame in buf.chunks_exact_mut(2) {
            if pos >= total {
                frame[0] = 0.0;
                frame[1] = 0.0;
                continue;
            }
            let l = self.pcm[pos * 2] as f64;
            let r = self.pcm[pos * 2 + 1] as f64;
            pos += 1;
            if !self.in_engine_eq {
                frame[0] = l;
                frame[1] = r;
                continue;
            }
            // Both buses always run (TB-12); equal-power blend between them.
            let (al, ar) = self.bus_a.process_frame(l, r);
            let (bl, br) = self.bus_b.process_frame(l, r);
            let (bl, br) = (bl * self.gain_b_extra, br * self.gain_b_extra);
            if self.xfade < target {
                self.xfade = (self.xfade + self.xstep).min(1.0);
            } else if self.xfade > target {
                self.xfade = (self.xfade - self.xstep).max(0.0);
            }
            let ga = (self.xfade * std::f64::consts::FRAC_PI_2).cos();
            let gb = (self.xfade * std::f64::consts::FRAC_PI_2).sin();
            frame[0] = al * ga + bl * gb;
            frame[1] = ar * ga + br * gb;
        }
        self.shared.pos.store(pos as u64, Ordering::Relaxed);
        pos < total
    }
}

/// Integrated LUFS (EBU R128) of the track as a given chain would play it —
/// true measurement, not the pink-weighted estimate (ADR-0003:17).
pub fn lufs_through(
    pcm: &[f32],
    rate: u32,
    specs: &[FilterSpec],
    preamp_db: f64,
) -> Result<f64, String> {
    let mut chain = ChainProcessor::new(specs, preamp_db, rate as f64);
    let mut meter =
        ebur128::EbuR128::new(2, rate, ebur128::Mode::I).map_err(|e| format!("ebur128: {e:?}"))?;
    let mut out = vec![0f64; 19_200];
    for block in pcm.chunks(19_200) {
        out.resize(block.len(), 0.0);
        for (i, fr) in block.chunks_exact(2).enumerate() {
            let (l, r) = chain.process_frame(fr[0] as f64, fr[1] as f64);
            out[i * 2] = l;
            out[i * 2 + 1] = r;
        }
        meter
            .add_frames_f64(&out[..block.len() - block.len() % 2])
            .map_err(|e| format!("ebur128: {e:?}"))?;
    }
    meter
        .loudness_global()
        .map_err(|e| format!("ebur128: {e:?}"))
}

/// Decode any audio file to interleaved stereo f32le PCM at `rate` via
/// ffmpeg — one canonical format, every source format, resampling included
/// (TB-07 resolved by construction). Cached per (track, rate).
pub fn ensure_pcm(
    ffmpeg: &Path,
    cache_dir: &Path,
    src: &Path,
    track_id: i64,
    rate: u32,
    on_decode_start: impl FnOnce(),
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(cache_dir).map_err(|e| e.to_string())?;
    let out = cache_dir.join(format!("{track_id}-{rate}.f32"));
    let fresh = match (out.metadata(), src.metadata()) {
        (Ok(o), Ok(s)) => match (o.modified(), s.modified()) {
            (Ok(om), Ok(sm)) => om >= sm && o.len() > 0,
            _ => false,
        },
        _ => false,
    };
    if fresh {
        return Ok(out);
    }
    on_decode_start();
    let tmp = out.with_extension("f32tmp");
    let output = std::process::Command::new(ffmpeg)
        .args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(src)
        .args(["-f", "f32le", "-ac", "2", "-ar", &rate.to_string()])
        .arg(&tmp)
        .output()
        .map_err(|e| format!("could not run ffmpeg: {e}"))?;
    if !output.status.success() {
        let _ = std::fs::remove_file(&tmp);
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(format!("decode failed: {}", err.trim()));
    }
    std::fs::rename(&tmp, &out).map_err(|e| e.to_string())?;
    Ok(out)
}

/// Load a PCM cache file into memory as f32 samples. A straight memcpy —
/// the cache is f32le and we only run on little-endian Windows, so byte
/// reinterpretation is exact (and instant even in debug builds).
pub fn load_pcm(path: &Path) -> Result<Arc<Vec<f32>>, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let n = bytes.len() / 4;
    let mut samples = vec![0f32; n];
    // SAFETY: samples has exactly n f32s (4n bytes); u8 → f32 copy is a
    // bit-level reinterpretation of IEEE-754 LE data written by ffmpeg.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), samples.as_mut_ptr() as *mut u8, n * 4);
    }
    Ok(Arc::new(samples))
}

/// A running playback session: raise `stop` to end it (the engine fades out
/// and releases the device — TB-25's lesson applied to hardware).
pub struct TrackSession {
    pub track_id: i64,
    pub stop: Arc<AtomicBool>,
    /// Raised by the audio thread when the stream has fully closed — the
    /// handoff to a new session waits on this so streams never overlap.
    pub done: Arc<AtomicBool>,
    pub shared: Arc<TrackShared>,
    pub mode: OutputMode,
}

impl TrackSession {
    pub fn new(
        track_id: i64,
        mode: OutputMode,
        rate: u32,
        total_frames: u64,
        start_frame: u64,
    ) -> Self {
        TrackSession {
            track_id,
            stop: Arc::new(AtomicBool::new(false)),
            done: Arc::new(AtomicBool::new(false)),
            shared: Arc::new(TrackShared {
                pos: AtomicU64::new(start_frame),
                paused: AtomicBool::new(false),
                side_b: AtomicBool::new(false),
                chain_gen: AtomicU64::new(0),
                bus: Mutex::new(BusConfig::default()),
                total_frames,
                rate,
            }),
            mode,
        }
    }
}

impl TrackSession {
    /// Spawn the audio thread. `on_end` runs when the stream closes
    /// (finished, stopped, or errored — the error is passed); `done` is
    /// raised first, so a handoff waiting on it never races the callback.
    pub fn spawn_audio(
        &self,
        pcm: Arc<Vec<f32>>,
        target_gain: f64,
        on_start: impl FnOnce(playback::StreamInfo) + Send + 'static,
        on_end: impl FnOnce(Option<String>) + Send + 'static,
    ) {
        let (shared, stop, done, mode) = (
            self.shared.clone(),
            self.stop.clone(),
            self.done.clone(),
            self.mode,
        );
        std::thread::spawn(move || {
            let mut src = TrackSource::new(pcm, shared);
            let mut on_start = Some(on_start);
            let result = playback::play(mode, &mut src, target_gain, &stop, |info| {
                if let Some(f) = on_start.take() {
                    f(info.clone());
                }
            });
            done.store(true, Ordering::Relaxed);
            on_end(result.err().map(|e| e.to_string()));
        });
    }
}
