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

    #[test]
    fn log_grid_spans_endpoints() {
        let f = log_freqs(20.0, 20000.0, 200);
        assert_eq!(f.len(), 200);
        assert!(close(f[0], 20.0, 1e-9) && close(f[199], 20000.0, 1e-6));
    }
}
