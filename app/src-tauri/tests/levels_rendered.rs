//! ADR-0016 end-to-end checks on RENDERED audio: real buffers pushed through the
//! actual track engine (`TrackSource`) and measured with the same LUFS meter the
//! trials use. No audio device, no APO writes. These assert the behavior we want;
//! the 2026-10-28 review (docs/research/loudness-matching-review-2026-10-28.md)
//! showed that unit tests on the math alone let real defects through.

#[allow(dead_code)]
#[path = "../src/engine.rs"]
mod engine;

use fletcher_core::config::FilterKind::*;
use fletcher_core::dsp::*;
use fletcher_core::playback::{OutputMode, Source};
use std::sync::{atomic::Ordering::Relaxed, Arc};

const FS: f64 = 48000.0;
const RANGE: (f64, f64) = (-120.0, 12.0);

fn tone(hz: f64, amp: f64, seconds: f64) -> Vec<f32> {
    (0..(seconds * FS) as usize)
        .flat_map(|i| {
            let x = (amp * (std::f64::consts::TAU * hz * i as f64 / FS).sin()) as f32;
            [x, x]
        })
        .collect()
}

/// Render `pcm` through the engine with `cfg`, on side B or A, from the start.
fn render(pcm: &Arc<Vec<f32>>, cfg: engine::BusConfig, side_b: bool) -> Vec<f64> {
    let frames = (pcm.len() / 2) as u64;
    let session = engine::TrackSession::new(0, OutputMode::Exclusive, 48000, frames, 0);
    *session.shared.bus.lock().unwrap() = cfg;
    session.shared.chain_gen.fetch_add(1, Relaxed);
    session.shared.side_b.store(side_b, Relaxed);
    let mut src = engine::TrackSource::new(pcm.clone(), session.shared.clone());
    src.set_rate(FS);
    let mut out = vec![0.0; pcm.len()];
    src.fill(&mut out);
    out
}

fn lufs(buf: &[f64]) -> f64 {
    let mut m = ebur128::EbuR128::new(2, 48000, ebur128::Mode::I).unwrap();
    let f: Vec<f32> = buf.iter().map(|&x| x as f32).collect();
    m.add_frames_f32(&f).unwrap();
    m.loudness_global().unwrap()
}

fn peak_dbfs(buf: &[f64]) -> f64 {
    20.0 * buf.iter().fold(0.0f64, |p, x| p.max(x.abs())).log10()
}

#[test]
fn switching_between_identical_buses_adds_no_level() {
    // Review F3: equal-power blending summed to √2 (+3 dB) mid-switch on
    // correlated audio, only on A↔B transitions: an ABX cue. Equal-gain can't.
    let pcm = Arc::new(vec![0.8f32; 4000]);
    let cfg = engine::BusConfig {
        matched: true,
        in_engine_eq: true,
        ..Default::default()
    };
    let session = engine::TrackSession::new(0, OutputMode::Exclusive, 48000, 2000, 0);
    *session.shared.bus.lock().unwrap() = cfg;
    session.shared.chain_gen.fetch_add(1, Relaxed);
    let mut src = engine::TrackSource::new(pcm, session.shared.clone());
    src.set_rate(FS);
    let mut out = vec![0.0; 400];
    src.fill(&mut out); // settled on A
    session.shared.side_b.store(true, Relaxed); // A → B, mid-stream
    let mut sw = vec![0.0; 1440];
    src.fill(&mut sw);
    let peak = sw.iter().fold(0.0f64, |p, x| p.max(x.abs()));
    let input = 0.8f32 as f64; // what the buffer actually holds
    assert!(
        peak <= input + 1e-12,
        "switch peaked at {peak} (input {input})"
    );
}

#[test]
fn clip_trial_correction_leaves_no_residual_at_the_rail() {
    // Review F1's case: B needs +5.3 dB but only has ~5 dB of headroom. A now
    // comes down by the rest, so the rendered A and B measure the same.
    let a = [FilterSpec {
        kind: LowShelfC,
        fc_hz: 1000.0,
        gain_db: 12.0,
        q: 0.7,
    }];
    let (pa, pb, _, matched) = pair_levels(
        plan_level(&a, 0.0, FS, 0.0, true, RANGE),
        plan_level(&[], 0.0, FS, 0.0, true, RANGE),
        RANGE,
    );
    assert!(matched);
    let pcm = Arc::new(tone(60.0, 0.9999, 4.0));
    let (la, _) = engine::lufs_peak_through(&pcm, 48000, &a, pa).unwrap();
    let (lb, peak_b) = engine::lufs_peak_through(&pcm, 48000, &[], pb).unwrap();
    let (ta, tb) = measured_trims(la, lb, peak_b, 0.3).unwrap();
    assert!(ta < 0.0, "this case must use A's share of the correction");
    let cfg = engine::BusConfig {
        specs: a.to_vec(),
        preamp_a_db: pa,
        specs_b: vec![],
        preamp_b_db: pb,
        trim_a_db: ta,
        trim_b_db: tb,
        matched: true,
        in_engine_eq: true,
    };
    let out_a = render(&pcm, cfg.clone(), false);
    let out_b = render(&pcm, cfg, true);
    let residual = lufs(&out_a) - lufs(&out_b);
    assert!(
        residual.abs() < 0.05,
        "rendered A/B residual {residual:.3} dB"
    );
    assert!(
        peak_dbfs(&out_b) <= -0.29,
        "B was raised past its headroom: {:.2} dBFS",
        peak_dbfs(&out_b)
    );
}

#[test]
fn paired_system_levels_render_equally_loud_on_pink_noise() {
    // The system path's contract (ADR-0016): equal by the K-weighted estimate.
    // Rendering real pink noise through both planned buses must agree with it.
    let a = vec![
        FilterSpec {
            kind: LowShelfC,
            fc_hz: 105.0,
            gain_db: 6.4,
            q: 0.7,
        },
        FilterSpec {
            kind: Peaking,
            fc_hz: 8800.0,
            gain_db: 5.1,
            q: 1.42,
        },
    ];
    let (pa, pb, _, matched) = pair_levels(
        plan_level(&a, 0.0, FS, -10.0, true, RANGE),
        plan_level(&[], 0.0, FS, -10.0, true, RANGE),
        RANGE,
    );
    assert!(matched);
    let mut noise =
        fletcher_core::signal::Signal::new(fletcher_core::signal::SignalKind::Pink, 0.25, FS, 3);
    let pcm: Arc<Vec<f32>> = Arc::new(
        (0..(20.0 * FS) as usize)
            .flat_map(|_| {
                let x = noise.next_sample() as f32;
                [x, x]
            })
            .collect(),
    );
    let cfg = engine::BusConfig {
        specs: a,
        preamp_a_db: pa,
        preamp_b_db: pb,
        matched: true,
        in_engine_eq: true,
        ..Default::default()
    };
    let diff = lufs(&render(&pcm, cfg.clone(), false)) - lufs(&render(&pcm, cfg, true));
    assert!(diff.abs() < 0.1, "pink-noise A/B residual {diff:.3} dB");
}

#[test]
fn silent_clips_are_refused_not_matched() {
    // Review F9: silence measures −∞ LUFS; the trim must be an error, not NaN.
    let pcm = vec![0.0f32; 288_000];
    let (la, pa) = engine::lufs_peak_through(&pcm, 48000, &[], -8.0).unwrap();
    let (lb, pb) = engine::lufs_peak_through(&pcm, 48000, &[], -8.0).unwrap();
    assert!(!la.is_finite() && !lb.is_finite());
    assert!(measured_trims(la, lb, pb.min(pa), 0.3).is_err());
}
