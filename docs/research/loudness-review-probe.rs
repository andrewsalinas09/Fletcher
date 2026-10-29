//! Offline review probe, 2026-10-28. No audio playback or APO writes.
//! Copy to app/src-tauri/tests/loudness_review_probe.rs and run
//! cargo test -p fletcher-app --test loudness_review_probe -- --nocapture
//! then remove that temporary copy. Assertions document current defects;
//! this is diagnostic evidence, not the desired-behavior regression suite.

#[allow(dead_code)]
#[path = "../src/engine.rs"]
mod engine;

use fletcher_core::config::FilterKind::*;
use fletcher_core::dsp::*;
use fletcher_core::playback::{OutputMode, Source};
use std::sync::{Arc, atomic::Ordering::Relaxed};

const FS: f64 = 48000.0;
const RANGE: (f64, f64) = (-80.0, 12.0);

fn tone(hz: f64, amp: f64) -> Vec<f32> {
    (0..144000).flat_map(|i| {
        let x = (amp * (std::f64::consts::TAU * hz * i as f64 / FS).sin()) as f32;
        [x, x]
    }).collect()
}

#[test]
fn pink_match_is_not_content_match() {
    let a = [FilterSpec { kind: LowShelfC, fc_hz: 105.0, gain_db: 8.0, q: 0.7 }];
    let (pa, pb, _) = pair_levels(
        plan_level(&a, FS, -8.0, 0.0, true, RANGE),
        plan_level(&[], FS, -8.0, 0.0, true, RANGE), RANGE);
    let pcm = tone(60.0, 0.1);
    let la = engine::lufs_through(&pcm, 48000, &a, pa).unwrap();
    let lb = engine::lufs_through(&pcm, 48000, &[], pb).unwrap();
    eprintln!("60 Hz, +8 dB bass shelf: pink estimate {:.4}, preamps {pa}/{pb}, measured mismatch {:.4} dB", loudness_db(&a, FS), la-lb);
    assert!(la - lb > 5.0);
}

#[test]
fn clip_trim_still_hits_an_unpaired_rail() {
    let a = [FilterSpec { kind: LowShelfC, fc_hz: 1000.0, gain_db: 12.0, q: 0.7 }];
    let (pa, pb, _) = pair_levels(
        plan_level(&a, FS, 0.0, 0.0, true, RANGE),
        plan_level(&[], FS, 0.0, 0.0, true, RANGE), RANGE);
    let pcm = tone(60.0, 0.9999);
    let (la, _) = engine::lufs_peak_through(&pcm, 48000, &a, pa).unwrap();
    let (lb, peak_b) = engine::lufs_peak_through(&pcm, 48000, &[], pb).unwrap();
    // Exact measured correction currently used in trial_material_clips.
    let want = la - lb;
    let trim = want.min((-peak_b - 0.3).max(0.0));
    let residual = la - (lb + trim);
    eprintln!("clip rail: A={la:.4}, B={lb:.4}, B peak={peak_b:.4}, desired trim={want:.4}, actual={trim:.4}, residual={residual:.4} dB");
    assert!(residual > 0.25);
}

#[test]
fn crossfade_adds_level_to_identical_signals() {
    let pcm = Arc::new(vec![0.8f32; 4000]);
    let session = engine::TrackSession::new(0, OutputMode::Exclusive, 48000, 2000, 0);
    session.shared.bus.lock().unwrap().in_engine_eq = true;
    let mut src = engine::TrackSource::new(pcm, session.shared.clone());
    src.set_rate(FS);
    session.shared.side_b.store(true, Relaxed);
    let mut out = vec![0.0; 1440];
    src.fill(&mut out);
    let peak = out.into_iter().fold(0.0f64, f64::max);
    eprintln!("identical 0.8 buses: crossfade peak={peak:.6}, boost={:.4} dB", 20.0*(peak/0.8).log10());
    assert!(peak > 1.13);
}

#[test]
fn sparse_peak_grid_misses_allowed_narrow_boosts() {
    let grid = log_freqs(20.0, 20000.0, 400);
    let fc = (grid[210] * grid[211]).sqrt();
    let chain = [FilterSpec { kind: Peaking, fc_hz: fc, gain_db: 12.0, q: 100.0 }];
    let p = auto_preamp_db(&chain, FS);
    let actual = chain_response_db(&chain, p, FS, &[fc])[0];
    eprintln!("narrow boost: Fc={fc:.4}, preamp={p:.4}, residual gain at center={actual:.4} dB");
    assert!(actual > 3.0);
}

#[test]
fn finite_preamp_floor_can_break_pairing() {
    let chain = vec![FilterSpec { kind: Peaking, fc_hz: 1000.0, gain_db: 30.0, q: 1.0 }; 3];
    let (pa, pb, below) = pair_levels(
        plan_level(&chain, FS, -50.0, 0.0, true, RANGE),
        plan_level(&[], FS, -50.0, 0.0, true, RANGE), RANGE);
    let mismatch = loudness_db(&chain, FS) + pa - pb;
    eprintln!("3x+30 PK, reference -30 / trim -20: pa={pa}, pb={pb}, shortfall={below}, mismatch={mismatch:.4} dB");
    assert!(mismatch > 40.0);
}

#[test]
fn silent_clips_produce_nonfinite_trims() {
    let pcm = vec![0.0f32; 288000];
    let (la, _) = engine::lufs_peak_through(&pcm, 48000, &[], -8.0).unwrap();
    let (lb, _) = engine::lufs_peak_through(&pcm, 48000, &[], -8.0).unwrap();
    let want = la - lb;
    eprintln!("silent clip: LUFS A={la}, LUFS B={lb}, trim={want}");
    assert!(!want.is_finite());
}
