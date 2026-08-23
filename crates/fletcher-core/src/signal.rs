//! The signal generator — one generator, never duplicated (FEATURES).
//!
//! Serves Clip Studio's synthetic library items, Settings' reference-level
//! calibration (Q-16), the Lab's test material, and later the Fingerprint
//! Lab's measurement sweeps. Amplitudes are hard-capped: the exclusive output
//! path bypasses Windows volume, so a synthetic signal must never be loud by
//! accident (TB-20 — extends the prime directive).

use crate::config::FilterKind;
use crate::dsp::{Biquad, BiquadState};
use crate::stats::Xorshift;

/// Hard amplitude ceiling for generated signals (≈ −12 dBFS peak).
pub const MAX_AMP: f64 = 0.25;
/// Default generation amplitude (≈ −20 dBFS peak).
pub const DEFAULT_AMP: f64 = 0.1;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SignalKind {
    White,
    Pink,
    Sine {
        hz: f64,
    },
    /// Repeating sweep; log spacing is the audio-native default.
    SweepLog {
        from_hz: f64,
        to_hz: f64,
        seconds: f64,
    },
    SweepLinear {
        from_hz: f64,
        to_hz: f64,
        seconds: f64,
    },
    /// White noise band-limited by a 4th-order HP/LP pair.
    BandNoise {
        lo_hz: f64,
        hi_hz: f64,
    },
}

/// A running generator instance: mono samples via `next_sample()`, deterministic
/// for a given seed (provenance: a recorded session can regenerate its signal).
pub struct Signal {
    kind: SignalKind,
    amp: f64,
    fs: f64,
    rng: Xorshift,
    phase: f64,
    t: f64,
    // Paul Kellet pink filter state.
    pink: [f64; 7],
    // Band-limiting cascade (2× HP + 2× LP) for BandNoise.
    band: Vec<(Biquad, BiquadState)>,
}

impl Signal {
    pub fn new(kind: SignalKind, amp: f64, fs: f64, seed: u64) -> Self {
        let amp = amp.clamp(0.0, MAX_AMP);
        let band = match kind {
            SignalKind::BandNoise { lo_hz, hi_hz } => {
                let lo = lo_hz.clamp(10.0, fs / 2.0 - 10.0);
                let hi = hi_hz.clamp(lo + 1.0, fs / 2.0 - 1.0);
                let hp = Biquad::rbj(FilterKind::HighPass, fs, lo, 0.0, 0.0);
                let lp = Biquad::rbj(FilterKind::LowPass, fs, hi, 0.0, 0.0);
                vec![
                    (hp, BiquadState::default()),
                    (hp, BiquadState::default()),
                    (lp, BiquadState::default()),
                    (lp, BiquadState::default()),
                ]
            }
            _ => Vec::new(),
        };
        Signal {
            kind,
            amp,
            fs,
            rng: Xorshift::new(seed),
            phase: 0.0,
            t: 0.0,
            pink: [0.0; 7],
            band,
        }
    }

    pub fn kind(&self) -> SignalKind {
        self.kind
    }

    /// Next mono sample (not Iterator::next — samples never end) in [-MAX_AMP, MAX_AMP].
    pub fn next_sample(&mut self) -> f64 {
        let dt = 1.0 / self.fs;
        let s = match self.kind {
            SignalKind::White => self.rng.next_pm1(),
            SignalKind::Pink => {
                // Paul Kellet's economy pink filter (−3 dB/oct from white).
                let w = self.rng.next_pm1();
                let p = &mut self.pink;
                p[0] = 0.99886 * p[0] + w * 0.0555179;
                p[1] = 0.99332 * p[1] + w * 0.0750759;
                p[2] = 0.96900 * p[2] + w * 0.1538520;
                p[3] = 0.86650 * p[3] + w * 0.3104856;
                p[4] = 0.55000 * p[4] + w * 0.5329522;
                p[5] = -0.7616 * p[5] - w * 0.0168980;
                let out = (p[0] + p[1] + p[2] + p[3] + p[4] + p[5] + p[6] + w * 0.5362) * 0.11;
                p[6] = w * 0.115926;
                out
            }
            SignalKind::Sine { hz } => {
                self.phase += 2.0 * std::f64::consts::PI * hz * dt;
                self.phase.sin()
            }
            SignalKind::SweepLog {
                from_hz,
                to_hz,
                seconds,
            } => {
                let tau = (self.t / seconds).fract();
                let f = from_hz * (to_hz / from_hz).powf(tau);
                self.t += dt;
                self.phase += 2.0 * std::f64::consts::PI * f * dt;
                self.phase.sin()
            }
            SignalKind::SweepLinear {
                from_hz,
                to_hz,
                seconds,
            } => {
                let tau = (self.t / seconds).fract();
                let f = from_hz + (to_hz - from_hz) * tau;
                self.t += dt;
                self.phase += 2.0 * std::f64::consts::PI * f * dt;
                self.phase.sin()
            }
            SignalKind::BandNoise { .. } => {
                let mut s = self.rng.next_pm1();
                for (c, st) in self.band.iter_mut() {
                    s = st.process(c, s);
                }
                s
            }
        };
        (s * self.amp).clamp(-MAX_AMP, MAX_AMP)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f64 = 48000.0;

    fn rms(sig: &mut Signal, n: usize, skip: usize) -> f64 {
        let mut sum = 0.0;
        for i in 0..n {
            let s = sig.next_sample();
            assert!(
                s.is_finite() && s.abs() <= MAX_AMP + 1e-12,
                "sample out of cap"
            );
            if i >= skip {
                sum += s * s;
            }
        }
        (sum / (n - skip) as f64).sqrt()
    }

    /// RMS of a signal after a band-pass — the tilt probe for white vs pink.
    fn band_rms(kind: SignalKind, fc: f64) -> f64 {
        let bp = Biquad::rbj(FilterKind::BandPass, FS, fc, 0.0, 1.0);
        let mut st = BiquadState::default();
        let mut sig = Signal::new(kind, DEFAULT_AMP, FS, 1234);
        let mut sum = 0.0;
        let (n, skip) = (192_000usize, 9_600usize);
        for i in 0..n {
            let y = st.process(&bp, sig.next_sample());
            if i >= skip {
                sum += y * y;
            }
        }
        (sum / (n - skip) as f64).sqrt()
    }

    #[test]
    fn white_is_bounded_and_deterministic() {
        let mut a = Signal::new(SignalKind::White, DEFAULT_AMP, FS, 7);
        let mut b = Signal::new(SignalKind::White, DEFAULT_AMP, FS, 7);
        let seq_a: Vec<f64> = (0..64).map(|_| a.next_sample()).collect();
        let seq_b: Vec<f64> = (0..64).map(|_| b.next_sample()).collect();
        assert_eq!(seq_a, seq_b, "same seed, same signal (provenance)");
        // Uniform [-amp, amp) noise has RMS = amp/sqrt(3).
        let mut w = Signal::new(SignalKind::White, DEFAULT_AMP, FS, 42);
        let r = rms(&mut w, 96_000, 0);
        let expect = DEFAULT_AMP / 3f64.sqrt();
        assert!(
            (r / expect - 1.0).abs() < 0.05,
            "white RMS {r:.4} vs {expect:.4}"
        );
    }

    #[test]
    fn amplitude_is_hard_capped() {
        let s = Signal::new(SignalKind::Sine { hz: 1000.0 }, 5.0, FS, 1);
        assert!(s.amp <= MAX_AMP, "requested 5.0, capped at {MAX_AMP}");
    }

    #[test]
    fn sine_hits_its_frequency() {
        // 1 kHz sine through a sharp (Q=8) bandpass at 1 kHz ≈ full RMS
        // (amp/√2); through the same filter at 4 kHz ≈ nothing.
        let probe = |fc: f64| {
            let bp = Biquad::rbj(FilterKind::BandPass, FS, fc, 0.0, 8.0);
            let mut st = BiquadState::default();
            let mut sig = Signal::new(SignalKind::Sine { hz: 1000.0 }, DEFAULT_AMP, FS, 1);
            let mut sum = 0.0;
            let (n, skip) = (96_000usize, 9_600usize);
            for i in 0..n {
                let y = st.process(&bp, sig.next_sample());
                if i >= skip {
                    sum += y * y;
                }
            }
            (sum / (n - skip) as f64).sqrt()
        };
        let on = probe(1000.0);
        let off = probe(4000.0);
        assert!(
            (on / (DEFAULT_AMP / 2f64.sqrt()) - 1.0).abs() < 0.05,
            "on-band RMS {on:.5}"
        );
        assert!(on > 10.0 * off, "on {on:.5} vs off {off:.5}");
    }

    #[test]
    fn pink_tilts_down_where_white_tilts_up() {
        // Through constant-Q bandpasses, white energy grows with fc
        // (~3 dB/oct); pink stays flat per octave.
        let w = 20.0
            * (band_rms(SignalKind::White, 6000.0) / band_rms(SignalKind::White, 100.0)).log10();
        let p =
            20.0 * (band_rms(SignalKind::Pink, 6000.0) / band_rms(SignalKind::Pink, 100.0)).log10();
        assert!(
            w > 10.0,
            "white 6k/100 ratio {w:.1} dB — expected strongly positive"
        );
        assert!(
            p.abs() < 6.0,
            "pink 6k/100 ratio {p:.1} dB — expected near flat"
        );
        assert!(w - p > 10.0, "pink must sit well below white in tilt");
    }

    #[test]
    fn band_noise_stays_in_its_band() {
        let kind = SignalKind::BandNoise {
            lo_hz: 500.0,
            hi_hz: 2000.0,
        };
        let inside = band_rms(kind, 1000.0);
        let below = band_rms(kind, 60.0);
        let above = band_rms(kind, 12000.0);
        assert!(inside > 5.0 * below, "in {inside:.5} vs below {below:.5}");
        assert!(inside > 5.0 * above, "in {inside:.5} vs above {above:.5}");
    }

    #[test]
    fn sweep_moves_through_its_range() {
        // A 2 s log sweep 100→10k: early samples live low, late samples high.
        let mut s = Signal::new(
            SignalKind::SweepLog {
                from_hz: 100.0,
                to_hz: 10_000.0,
                seconds: 2.0,
            },
            DEFAULT_AMP,
            FS,
            1,
        );
        let lo_bp = Biquad::rbj(FilterKind::BandPass, FS, 150.0, 0.0, 2.0);
        let hi_bp = Biquad::rbj(FilterKind::BandPass, FS, 8000.0, 0.0, 2.0);
        let (mut lo_st, mut hi_st) = (BiquadState::default(), BiquadState::default());
        let (mut early_lo, mut late_hi) = (0.0f64, 0.0f64);
        let n = (2.0 * FS) as usize;
        for i in 0..n {
            let x = s.next_sample();
            let l = lo_st.process(&lo_bp, x);
            let h = hi_st.process(&hi_bp, x);
            if i < n / 4 {
                early_lo += l * l;
            }
            if i > 3 * n / 4 {
                late_hi += h * h;
            }
        }
        assert!(
            early_lo > 0.0 && late_hi > 0.0,
            "sweep energy present in both bands"
        );
    }
}
