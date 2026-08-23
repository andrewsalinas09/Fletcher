//! Filter math: RBJ-cookbook biquad coefficients and frequency response.
//!
//! Shared by the curve renderer, auto-preamp, and (later) the track engine —
//! the UI never reimplements DSP (ADR-0001/0005).

use crate::config::FilterKind;

const DEFAULT_Q: f64 = std::f64::consts::FRAC_1_SQRT_2;

/// Normalized biquad coefficients (a0 divided out).
#[derive(Debug, Clone, Copy)]
pub struct Biquad {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl Biquad {
    /// RBJ Audio EQ Cookbook coefficients. `gain_db`/`q` fall back to
    /// 0 dB / 0.7071 where a filter type doesn't take them.
    pub fn rbj(kind: FilterKind, fs: f64, fc: f64, gain_db: f64, q: f64) -> Self {
        use FilterKind::*;
        let a = 10f64.powf(gain_db / 40.0);
        let w0 = 2.0 * std::f64::consts::PI * fc / fs;
        let (sin_w, cos_w) = w0.sin_cos();
        let q = match kind {
            LowPass | HighPass | LowShelf | HighShelf => DEFAULT_Q,
            _ => q,
        };
        let alpha = sin_w / (2.0 * q);
        let sqrt_a2alpha = 2.0 * a.sqrt() * alpha;

        let (b0, b1, b2, a0, a1, a2) = match kind {
            LowPass | LowPassQ => {
                let b1 = 1.0 - cos_w;
                (
                    b1 / 2.0,
                    b1,
                    b1 / 2.0,
                    1.0 + alpha,
                    -2.0 * cos_w,
                    1.0 - alpha,
                )
            }
            HighPass | HighPassQ => {
                let b1 = -(1.0 + cos_w);
                (
                    -b1 / 2.0,
                    b1,
                    -b1 / 2.0,
                    1.0 + alpha,
                    -2.0 * cos_w,
                    1.0 - alpha,
                )
            }
            BandPass => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cos_w, 1.0 - alpha),
            Notch => (
                1.0,
                -2.0 * cos_w,
                1.0,
                1.0 + alpha,
                -2.0 * cos_w,
                1.0 - alpha,
            ),
            AllPass => (
                1.0 - alpha,
                -2.0 * cos_w,
                1.0 + alpha,
                1.0 + alpha,
                -2.0 * cos_w,
                1.0 - alpha,
            ),
            Peaking => (
                1.0 + alpha * a,
                -2.0 * cos_w,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cos_w,
                1.0 - alpha / a,
            ),
            LowShelf | LowShelfC => (
                a * ((a + 1.0) - (a - 1.0) * cos_w + sqrt_a2alpha),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w),
                a * ((a + 1.0) - (a - 1.0) * cos_w - sqrt_a2alpha),
                (a + 1.0) + (a - 1.0) * cos_w + sqrt_a2alpha,
                -2.0 * ((a - 1.0) + (a + 1.0) * cos_w),
                (a + 1.0) + (a - 1.0) * cos_w - sqrt_a2alpha,
            ),
            HighShelf | HighShelfC => (
                a * ((a + 1.0) + (a - 1.0) * cos_w + sqrt_a2alpha),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w),
                a * ((a + 1.0) + (a - 1.0) * cos_w - sqrt_a2alpha),
                (a + 1.0) - (a - 1.0) * cos_w + sqrt_a2alpha,
                2.0 * ((a - 1.0) - (a + 1.0) * cos_w),
                (a + 1.0) - (a - 1.0) * cos_w - sqrt_a2alpha,
            ),
        };
        Biquad {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }

    /// Magnitude response in dB at frequency `f` (Hz).
    pub fn magnitude_db(&self, f: f64, fs: f64) -> f64 {
        let w = 2.0 * std::f64::consts::PI * f / fs;
        let (s1, c1) = w.sin_cos();
        let (s2, c2) = (2.0 * w).sin_cos();
        // H(e^jw) evaluated as complex num/den; z^-1 = e^{-jw}.
        let num_re = self.b0 + self.b1 * c1 + self.b2 * c2;
        let num_im = -(self.b1 * s1 + self.b2 * s2);
        let den_re = 1.0 + self.a1 * c1 + self.a2 * c2;
        let den_im = -(self.a1 * s1 + self.a2 * s2);
        let mag2 = (num_re * num_re + num_im * num_im) / (den_re * den_re + den_im * den_im);
        10.0 * mag2.log10()
    }
}

/// Per-instance filter memory for realtime processing (direct form II
/// transposed). `Biquad` itself stays stateless/`Copy` — response-math
/// callers never pay for state they don't use.
#[derive(Debug, Clone, Copy, Default)]
pub struct BiquadState {
    z1: f64,
    z2: f64,
}

impl BiquadState {
    #[inline]
    pub fn process(&mut self, c: &Biquad, x: f64) -> f64 {
        let y = c.b0 * x + self.z1;
        self.z1 = c.b1 * x - c.a1 * y + self.z2;
        self.z2 = c.b2 * x - c.a2 * y;
        y
    }
}

/// A realtime chain: preamp gain + biquad cascade with per-channel state.
/// One instance per bus; stereo frames flow through `process_frame`.
pub struct ChainProcessor {
    coeffs: Vec<Biquad>,
    state: Vec<[BiquadState; 2]>,
    gain: f64,
}

impl ChainProcessor {
    pub fn new(filters: &[FilterSpec], preamp_db: f64, fs: f64) -> Self {
        ChainProcessor {
            coeffs: filters.iter().map(|f| f.biquad(fs)).collect(),
            state: vec![[BiquadState::default(); 2]; filters.len()],
            gain: 10f64.powf(preamp_db / 20.0),
        }
    }

    /// Swap coefficients mid-stream (live edits). Existing per-filter state is
    /// kept where the cascade length allows — a tweak shouldn't reset the tail.
    pub fn set_chain(&mut self, filters: &[FilterSpec], preamp_db: f64, fs: f64) {
        self.coeffs = filters.iter().map(|f| f.biquad(fs)).collect();
        self.state
            .resize(self.coeffs.len(), [BiquadState::default(); 2]);
        self.gain = 10f64.powf(preamp_db / 20.0);
    }

    #[inline]
    pub fn process_frame(&mut self, l: f64, r: f64) -> (f64, f64) {
        let mut l = l * self.gain;
        let mut r = r * self.gain;
        for (c, st) in self.coeffs.iter().zip(self.state.iter_mut()) {
            l = st[0].process(c, l);
            r = st[1].process(c, r);
        }
        (l, r)
    }
}

/// One filter in a response chain.
#[derive(Debug, Clone, Copy)]
pub struct FilterSpec {
    pub kind: FilterKind,
    pub fc_hz: f64,
    pub gain_db: f64,
    pub q: f64,
}

impl FilterSpec {
    pub fn biquad(&self, fs: f64) -> Biquad {
        Biquad::rbj(self.kind, fs, self.fc_hz, self.gain_db, self.q)
    }
}

/// Log-spaced frequency grid from `lo` to `hi` Hz.
pub fn log_freqs(lo: f64, hi: f64, n: usize) -> Vec<f64> {
    let (llo, lhi) = (lo.log10(), hi.log10());
    (0..n)
        .map(|i| 10f64.powf(llo + (lhi - llo) * i as f64 / (n - 1) as f64))
        .collect()
}

/// Preamp that guarantees the summed chain never exceeds 0 dB (TB-06):
/// the negative of the summed response's positive peak — filter *interaction*
/// counts, not just the largest single gain. Rounded to 0.1 dB.
pub fn auto_preamp_db(filters: &[FilterSpec], fs: f64) -> f64 {
    let freqs = log_freqs(20.0, 20000.0, 400);
    let peak = chain_response_db(filters, 0.0, fs, &freqs)
        .into_iter()
        .fold(0.0_f64, f64::max);
    -(peak * 10.0).ceil() / 10.0
}

/// Summed chain response (preamp + all filters) at each frequency.
pub fn chain_response_db(
    filters: &[FilterSpec],
    preamp_db: f64,
    fs: f64,
    freqs: &[f64],
) -> Vec<f64> {
    let biquads: Vec<Biquad> = filters.iter().map(|f| f.biquad(fs)).collect();
    freqs
        .iter()
        .map(|&f| preamp_db + biquads.iter().map(|b| b.magnitude_db(f, fs)).sum::<f64>())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::FilterKind::*;

    const FS: f64 = 48000.0;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() < tol
    }

    #[test]
    fn peaking_hits_its_gain_at_center() {
        for gain in [-6.0, -1.7, 3.0, 6.4] {
            let b = Biquad::rbj(Peaking, FS, 1000.0, gain, 1.0);
            assert!(
                close(b.magnitude_db(1000.0, FS), gain, 0.01),
                "PK {gain} dB at fc"
            );
        }
    }

    #[test]
    fn lowpass_is_minus_3db_at_corner_and_flat_below() {
        let b = Biquad::rbj(LowPass, FS, 1000.0, 0.0, 0.0);
        assert!(close(b.magnitude_db(1000.0, FS), -3.01, 0.05));
        assert!(close(b.magnitude_db(20.0, FS), 0.0, 0.01));
        assert!(b.magnitude_db(10000.0, FS) < -35.0);
    }

    #[test]
    fn shelves_reach_gain_in_their_band() {
        let ls = Biquad::rbj(LowShelfC, FS, 105.0, 6.4, 0.7);
        assert!(
            close(ls.magnitude_db(10.0, FS), 6.4, 0.05),
            "low shelf floor"
        );
        assert!(
            close(ls.magnitude_db(20000.0, FS), 0.0, 0.05),
            "low shelf top"
        );
        let hs = Biquad::rbj(HighShelfC, FS, 10000.0, -2.1, 0.7);
        assert!(
            close(hs.magnitude_db(22000.0, FS), -2.1, 0.15),
            "high shelf top"
        );
        assert!(
            close(hs.magnitude_db(100.0, FS), 0.0, 0.05),
            "high shelf floor"
        );
    }

    #[test]
    fn notch_kills_center_leaves_far_field() {
        let b = Biquad::rbj(Notch, FS, 1000.0, 0.0, 10.0);
        assert!(b.magnitude_db(1000.0, FS) < -40.0);
        assert!(close(b.magnitude_db(50.0, FS), 0.0, 0.05));
    }

    #[test]
    fn chain_sums_preamp_and_filters() {
        let filters = [FilterSpec {
            kind: Peaking,
            fc_hz: 1000.0,
            gain_db: 6.0,
            q: 1.0,
        }];
        let r = chain_response_db(&filters, -8.1, FS, &[1000.0]);
        assert!(close(r[0], -2.1, 0.02));
    }

    /// The realtime path validates against the already-trusted frequency-domain
    /// path: a sine driven through `BiquadState` must settle to the RMS gain
    /// `magnitude_db` predicts. No golden data — the two derivations must agree.
    #[test]
    fn time_domain_matches_frequency_response() {
        // Frequencies chosen so the measurement window holds a whole number of
        // cycles (f * 0.9 is an integer) — no leakage in the RMS.
        for (kind, fc, gain, q, f) in [
            (Peaking, 1000.0, 6.0, 1.0, 1000.0),
            (Peaking, 100.0, -4.3, 2.0, 100.0),
            (LowShelfC, 105.0, 6.4, 0.7, 30.0),
            (HighShelfC, 8000.0, -3.0, 0.7, 16000.0),
            (LowPass, 1000.0, 0.0, 0.0, 4000.0),
        ] {
            let b = Biquad::rbj(kind, FS, fc, gain, q);
            let mut st = BiquadState::default();
            let (n, settle) = (48000usize, 4800usize);
            let (mut sum_in, mut sum_out) = (0.0f64, 0.0f64);
            for i in 0..n {
                let x = (2.0 * std::f64::consts::PI * f * i as f64 / FS).sin();
                let y = st.process(&b, x);
                assert!(y.is_finite(), "{kind:?} produced non-finite output");
                if i >= settle {
                    sum_in += x * x;
                    sum_out += y * y;
                }
            }
            let measured = 10.0 * (sum_out / sum_in).log10();
            let predicted = b.magnitude_db(f, FS);
            assert!(
                close(measured, predicted, 0.1),
                "{kind:?} @ {f} Hz: time {measured:.3} dB vs freq {predicted:.3} dB"
            );
        }
    }

    #[test]
    fn chain_processor_applies_preamp_and_cascade() {
        let filters = [
            FilterSpec {
                kind: Peaking,
                fc_hz: 1000.0,
                gain_db: 6.0,
                q: 1.0,
            },
            FilterSpec {
                kind: Peaking,
                fc_hz: 1000.0,
                gain_db: -2.0,
                q: 1.0,
            },
        ];
        let mut chain = ChainProcessor::new(&filters, -8.1, FS);
        let f = 1000.0;
        let (n, settle) = (48000usize, 4800usize);
        let (mut sum_in, mut sum_out) = (0.0f64, 0.0f64);
        for i in 0..n {
            let x = (2.0 * std::f64::consts::PI * f * i as f64 / FS).sin();
            let (l, r) = chain.process_frame(x, x);
            assert!(close(l, r, 1e-12), "identical channels stay identical");
            if i >= settle {
                sum_in += x * x;
                sum_out += l * l;
            }
        }
        let measured = 10.0 * (sum_out / sum_in).log10();
        let predicted = chain_response_db(&filters, -8.1, FS, &[f])[0];
        assert!(
            close(measured, predicted, 0.1),
            "chain: time {measured:.3} dB vs freq {predicted:.3} dB"
        );
    }

    #[test]
    fn log_grid_spans_endpoints() {
        let f = log_freqs(20.0, 20000.0, 200);
        assert_eq!(f.len(), 200);
        assert!(close(f[0], 20.0, 1e-9) && close(f[199], 20000.0, 1e-6));
    }
}
