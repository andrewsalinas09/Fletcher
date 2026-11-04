//! Frequency responses: a headphone's measurement, a target, preference bounds,
//! and how they line up with the EQ on one graph (ADR-0021).
//!
//! Everything follows the research in docs/research/targets-and-preference-bounds.md:
//! - A measurement is anchored on its mean over the third-octave band at
//!   500 Hz (IEC 60268-7; how Harman presents targets).
//! - A target is moved to the measurement, never the reverse, by the
//!   least-squares offset over 100 Hz–10 kHz (AutoEq's published method; the
//!   range ear simulators are specified for). A median option is offered.
//! - The predicted response is measurement + EQ and is never refitted, so it
//!   shows the EQ's real gain and preamp.
//! - Left and right share one offset, from their amplitude average, so a real
//!   imbalance between them stays visible.
//! - A target can be a base response plus four knobs (bass, ear gain, treble,
//!   tilt), the shape squig.link and AutoEq use.

use crate::config::FilterKind;
use crate::dsp::Biquad;

/// AutoEq's frequency grid: 20 Hz upward in ×1.01 steps, up to 20 kHz
/// (its `DEFAULT_F_MIN`, `DEFAULT_F_MAX`, `DEFAULT_STEP`). Rounded to 0.01 Hz
/// the way its files print it.
pub fn autoeq_grid() -> Vec<f64> {
    let mut out = Vec::new();
    let mut f = 20.0_f64;
    while f <= 20000.0 {
        out.push((f * 100.0).round() / 100.0);
        f *= 1.01;
    }
    out
}

/// A response: dB at increasing frequencies (Hz).
#[derive(Debug, Clone, PartialEq)]
pub struct Curve {
    pub freqs: Vec<f64>,
    pub db: Vec<f64>,
}

/// How to smooth a curve for display (and, later, for fitting).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Smoothing {
    None,
    /// 1/N octave, e.g. `Octave(12.0)` for 1/12 octave.
    Octave(f64),
    /// One equivalent rectangular bandwidth (Glasberg & Moore): about 1 octave
    /// at 50 Hz and 1/6 octave above 1 kHz.
    Erb,
}

/// Points per octave on the even log grid used for averaging and smoothing.
const PPO: f64 = 96.0;

impl Curve {
    /// Read a response file: AutoEq's CSV (`frequency,raw,…`, by header) or a
    /// REW text export (`*` comment lines, then frequency and SPL separated by
    /// tabs or spaces, with commas as decimal points in some locales). Any
    /// further columns (phase, …) are ignored.
    pub fn parse(text: &str) -> Result<Curve, String> {
        let mut column = 1;
        let mut points: Vec<(f64, f64)> = Vec::new();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty()
                || line.starts_with('*')
                || line.starts_with('#')
                || line.starts_with("//")
            {
                continue;
            }
            let fields: Vec<String> = if line.contains(['\t', ';', ' ']) {
                line.split(['\t', ';', ' '])
                    .filter(|t| !t.is_empty())
                    .map(|t| t.replace(',', "."))
                    .collect()
            } else {
                line.split(',').map(|t| t.trim().to_string()).collect()
            };
            let nums: Vec<Option<f64>> = fields.iter().map(|t| t.parse::<f64>().ok()).collect();
            if nums.first().copied().flatten().is_none() {
                // A header: AutoEq names its measured column "raw".
                if let Some(i) = fields.iter().position(|t| t.eq_ignore_ascii_case("raw")) {
                    column = i;
                }
                continue;
            }
            if let (Some(Some(f)), Some(Some(db))) = (nums.first(), nums.get(column))
                && f.is_finite()
                && *f > 0.0
                && db.is_finite()
            {
                points.push((*f, *db));
            }
        }
        points.sort_by(|a, b| a.0.total_cmp(&b.0));
        points.dedup_by(|a, b| a.0 == b.0);
        if points.len() < 2 {
            return Err("no frequency response found: expected lines of frequency and dB".into());
        }
        Ok(Curve {
            freqs: points.iter().map(|p| p.0).collect(),
            db: points.iter().map(|p| p.1).collect(),
        })
    }

    /// The level at `f`, interpolated linearly in log-frequency. `None`
    /// outside the measured range: nothing is invented past the data.
    pub fn at(&self, f: f64) -> Option<f64> {
        let (first, last) = (*self.freqs.first()?, *self.freqs.last()?);
        if !(first..=last).contains(&f) {
            return None;
        }
        let i = self.freqs.partition_point(|&x| x < f);
        if i == 0 {
            return Some(self.db[0]);
        }
        let (f0, f1) = (self.freqs[i - 1], self.freqs[i]);
        let t = (f / f0).ln() / (f1 / f0).ln();
        Some(self.db[i - 1] + t * (self.db[i] - self.db[i - 1]))
    }

    /// The curve at each of `freqs` (`None` outside its range).
    pub fn sample(&self, freqs: &[f64]) -> Vec<Option<f64>> {
        freqs.iter().map(|&f| self.at(f)).collect()
    }

    pub fn shifted(&self, db: f64) -> Curve {
        Curve {
            freqs: self.freqs.clone(),
            db: self.db.iter().map(|v| v + db).collect(),
        }
    }

    /// The curve resampled to an even log grid (PPO points per octave) over
    /// its own range: every octave then weighs the same in an average.
    fn even(&self) -> Curve {
        let (lo, hi) = (self.freqs[0], *self.freqs.last().unwrap());
        let n = ((hi / lo).log2() * PPO).floor() as usize + 1;
        let freqs: Vec<f64> = (0..n).map(|k| lo * 2f64.powf(k as f64 / PPO)).collect();
        let db = freqs.iter().map(|&f| self.at(f).unwrap_or(0.0)).collect();
        Curve { freqs, db }
    }

    /// Smoothed: each point becomes the mean (in dB) of the curve within the
    /// window around it, on an even log grid. The result is on that grid.
    pub fn smoothed(&self, s: Smoothing) -> Curve {
        if s == Smoothing::None || self.freqs.len() < 3 {
            return self.clone();
        }
        let even = self.even();
        let n = even.db.len();
        let mut prefix = vec![0.0; n + 1];
        for (i, v) in even.db.iter().enumerate() {
            prefix[i + 1] = prefix[i] + v;
        }
        let db = even
            .freqs
            .iter()
            .enumerate()
            .map(|(i, &f)| {
                let width_oct = match s {
                    Smoothing::Octave(frac) => 1.0 / frac,
                    Smoothing::Erb => {
                        let erb = 24.7 * (4.37 * f / 1000.0 + 1.0);
                        let lo = (f - erb / 2.0).max(f / 4.0);
                        (f + erb / 2.0).log2() - lo.log2()
                    }
                    Smoothing::None => 0.0,
                };
                let half = ((width_oct / 2.0) * PPO).round() as usize;
                let (a, b) = (i.saturating_sub(half), (i + half).min(n - 1));
                (prefix[b + 1] - prefix[a]) / (b + 1 - a) as f64
            })
            .collect();
        Curve {
            freqs: even.freqs,
            db,
        }
    }

    /// The mean level over `lo`–`hi` Hz, every octave weighed the same.
    /// `None` when the curve doesn't cover the band.
    pub fn band_mean(&self, lo: f64, hi: f64) -> Option<f64> {
        let vals = band_values(lo, hi, |f| self.at(f))?;
        Some(vals.iter().sum::<f64>() / vals.len() as f64)
    }
}

/// `value(f)` on an even log grid across `lo`–`hi`; `None` if any point is missing.
fn band_values(lo: f64, hi: f64, value: impl Fn(f64) -> Option<f64>) -> Option<Vec<f64>> {
    let n = ((hi / lo).log2() * PPO).floor() as usize + 1;
    (0..n)
        .map(|k| value(lo * 2f64.powf(k as f64 / PPO)))
        .collect()
}

/// The anchor band: the third octave centered on 500 Hz.
pub const ANCHOR_HZ: f64 = 500.0;
/// The band a target is fitted over.
pub const FIT_LO_HZ: f64 = 100.0;
pub const FIT_HI_HZ: f64 = 10_000.0;
/// Above this, a measurement depends more on fit and seating than on the
/// headphone (reseating moves it 8–15 dB): shown as low confidence.
pub const CONFIDENT_BELOW_HZ: f64 = 10_000.0;

/// The offset (dB) that puts `curve`'s third-octave band at 500 Hz at 0 dB.
pub fn anchor_offset(curve: &Curve) -> Option<f64> {
    let k = 2f64.powf(1.0 / 6.0);
    curve.band_mean(ANCHOR_HZ / k, ANCHOR_HZ * k).map(|m| -m)
}

/// The offset (dB) that best moves `target` onto `to` over 100 Hz–10 kHz:
/// the mean difference (least squares for a constant), or with `robust` the
/// median, which ignores a few sharp resonances. Every octave weighs the same.
pub fn fit_offset(target: &Curve, to: &Curve, robust: bool) -> Option<f64> {
    let mut diffs = band_values(FIT_LO_HZ, FIT_HI_HZ, |f| Some(to.at(f)? - target.at(f)?))?;
    if robust {
        diffs.sort_by(f64::total_cmp);
        let m = diffs.len() / 2;
        Some(if diffs.len() % 2 == 1 {
            diffs[m]
        } else {
            (diffs[m - 1] + diffs[m]) / 2.0
        })
    } else {
        Some(diffs.iter().sum::<f64>() / diffs.len() as f64)
    }
}

/// A target's four adjustments, applied to a base response.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TargetKnobs {
    /// Low shelf at 105 Hz, Q 0.7 (AutoEq's bass boost; squig.link's "Bass").
    pub bass_db: f64,
    /// Peak at 2.75 kHz, Q 1 (squig.link's "Ear gain").
    pub ear_db: f64,
    /// High shelf at 2.5 kHz, Q 0.42 (squig.link's "Treble").
    pub treble_db: f64,
    /// dB per octave, pivoting at 632 Hz, the middle of 20 Hz–20 kHz (as
    /// AutoEq's `log_tilt`). Where it pivots only moves the whole curve, which
    /// the fit to the measurement removes anyway.
    pub tilt_db_per_oct: f64,
}

pub const BASS_HZ: f64 = 105.0;
pub const BASS_Q: f64 = 0.7;
pub const EAR_HZ: f64 = 2750.0;
pub const EAR_Q: f64 = 1.0;
pub const TREBLE_HZ: f64 = 2500.0;
pub const TREBLE_Q: f64 = 0.42;
/// AutoEq's tilt center: 20 · √(20000 / 20) Hz.
const TILT_PIVOT_HZ: f64 = 632.455_532_033_675_9;
/// A target is an ideal shape, not something played: its shelves are
/// evaluated well below Nyquist so no sample rate bends their top end.
const KNOB_FS: f64 = 384_000.0;

impl TargetKnobs {
    /// The knobs' total change at `f`, in dB. AutoEq's bass boost is the RBJ
    /// low shelf with fc as its midpoint (half the gain at 105 Hz), i.e. APO's
    /// LSC; its treble boost likewise.
    pub fn db_at(&self, f: f64) -> f64 {
        let shelf = |kind, fc, gain, q| {
            if gain == 0.0 {
                0.0
            } else {
                Biquad::rbj(kind, KNOB_FS, fc, gain, q).magnitude_db(f, KNOB_FS)
            }
        };
        shelf(FilterKind::LowShelfC, BASS_HZ, self.bass_db, BASS_Q)
            + shelf(FilterKind::Peaking, EAR_HZ, self.ear_db, EAR_Q)
            + shelf(FilterKind::HighShelfC, TREBLE_HZ, self.treble_db, TREBLE_Q)
            + self.tilt_db_per_oct * (f / TILT_PIVOT_HZ).log2()
    }

    /// `base` with the knobs applied.
    pub fn apply(&self, base: &Curve) -> Curve {
        Curve {
            freqs: base.freqs.clone(),
            db: base
                .freqs
                .iter()
                .zip(&base.db)
                .map(|(&f, v)| v + self.db_at(f))
                .collect(),
        }
    }
}

/// Preference bounds: the band's upper and lower edges as offsets (dB) from
/// the target they surround.
#[derive(Debug, Clone, PartialEq)]
pub struct Bounds {
    pub upper: Curve,
    pub lower: Curve,
}

/// A headphone's measurement: one curve, or a curve per ear.
#[derive(Debug, Clone, PartialEq)]
pub struct Measurement {
    pub left: Curve,
    pub right: Option<Curve>,
}

impl Measurement {
    /// Both ears averaged in amplitude (as CrinGraph does), on the left's grid.
    pub fn average(&self) -> Curve {
        let Some(right) = &self.right else {
            return self.left.clone();
        };
        let db = self
            .left
            .freqs
            .iter()
            .zip(&self.left.db)
            .map(|(&f, &l)| match right.at(f) {
                Some(r) => 20.0 * ((10f64.powf(l / 20.0) + 10f64.powf(r / 20.0)) / 2.0).log10(),
                None => l,
            })
            .collect();
        Curve {
            freqs: self.left.freqs.clone(),
            db,
        }
    }
}

/// Everything drawn on the EQ graph, at the graph's own frequencies. `None`
/// marks points outside a curve's data (AutoEq's start at 20 Hz; the graph at 10).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Overlay {
    pub measurement_left: Vec<Option<f64>>,
    /// Only when the measurement has separate ears.
    pub measurement_right: Option<Vec<Option<f64>>>,
    /// Measurement + EQ, per ear (right only when either differs per ear).
    pub predicted_left: Vec<Option<f64>>,
    pub predicted_right: Option<Vec<Option<f64>>>,
    pub target: Vec<Option<f64>>,
    pub bounds_upper: Vec<Option<f64>>,
    pub bounds_lower: Vec<Option<f64>>,
    /// The offset the target was moved by to meet the measurement (dB).
    pub target_offset_db: Option<f64>,
}

/// What goes into an overlay.
pub struct OverlayInput<'a> {
    pub freqs: &'a [f64],
    pub measurement: Option<&'a Measurement>,
    pub target: Option<&'a Curve>,
    pub bounds: Option<&'a Bounds>,
    pub smoothing: Smoothing,
    /// The full EQ as APO plays it, per ear, at `freqs` (`eq_right` None = same).
    pub eq_left: &'a [f64],
    pub eq_right: Option<&'a [f64]>,
    /// Fit with the median instead of the mean.
    pub robust_fit: bool,
}

/// Line everything up on one dB axis: the measurement at 0 dB at 500 Hz, the
/// target fitted to it, the bounds around the target, and measurement + EQ
/// with no refit.
pub fn overlay(input: &OverlayInput) -> Overlay {
    let freqs = input.freqs;
    let mut out = Overlay::default();

    let ears = input.measurement.map(|m| {
        let offset = anchor_offset(&m.average()).unwrap_or(0.0);
        let left = m.left.smoothed(input.smoothing).shifted(offset);
        let right = m
            .right
            .as_ref()
            .map(|r| r.smoothed(input.smoothing).shifted(offset));
        let avg = Measurement {
            left: left.clone(),
            right: right.clone(),
        }
        .average();
        (left, right, avg)
    });

    if let Some((left, right, _)) = &ears {
        out.measurement_left = left.sample(freqs);
        out.measurement_right = right.as_ref().map(|r| r.sample(freqs));
        let plus = |m: &Curve, eq: &[f64]| -> Vec<Option<f64>> {
            m.sample(freqs)
                .iter()
                .zip(eq)
                .map(|(v, e)| v.map(|v| v + e))
                .collect()
        };
        out.predicted_left = plus(left, input.eq_left);
        let right_meas = right.as_ref().unwrap_or(left);
        if right.is_some() || input.eq_right.is_some() {
            out.predicted_right = Some(plus(right_meas, input.eq_right.unwrap_or(input.eq_left)));
        }
    }

    if let Some(target) = input.target {
        let offset = match &ears {
            Some((_, _, avg)) => fit_offset(target, avg, input.robust_fit),
            None => anchor_offset(target),
        };
        let placed = target.shifted(offset.unwrap_or(0.0));
        out.target_offset_db = offset;
        out.target = placed.sample(freqs);
        if let Some(b) = input.bounds {
            let edge = |e: &Curve| -> Vec<Option<f64>> {
                freqs
                    .iter()
                    .map(|&f| Some(placed.at(f)? + e.at(f)?))
                    .collect()
            };
            out.bounds_upper = edge(&b.upper);
            out.bounds_lower = edge(&b.lower);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Curve {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        Curve::parse(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn flat(db: f64) -> Curve {
        let freqs = autoeq_grid();
        Curve {
            db: vec![db; freqs.len()],
            freqs,
        }
    }

    #[test]
    fn the_grid_is_autoeqs() {
        let hd650 = fixture("autoeq-oratory1990-hd650.csv");
        let grid = autoeq_grid();
        assert_eq!(grid.len(), hd650.freqs.len());
        for (a, b) in grid.iter().zip(&hd650.freqs) {
            assert!((a - b).abs() < 0.011, "{a} vs {b}");
        }
    }

    #[test]
    fn parses_autoeq_csv_by_its_raw_column() {
        let c = Curve::parse("frequency,smoothed,raw\n20.0,1.0,-6.9\n20.2,1.1,-6.8\n").unwrap();
        assert_eq!(c.db, vec![-6.9, -6.8]);
    }

    #[test]
    fn parses_rew_text_with_comma_decimals_and_comments() {
        let text = "* Measurement data measured by REW V5.31\n* Freq(Hz)\tSPL(dB)\tPhase(degrees)\n\
                    20,000\t75,500\t-12,3\n20,250\t75,750\t-12,1\n1000,000\t80,000\t0,0\n";
        let c = Curve::parse(text).unwrap();
        assert_eq!(c.freqs, vec![20.0, 20.25, 1000.0]);
        assert_eq!(c.db, vec![75.5, 75.75, 80.0]);
        assert!(Curve::parse("nothing here").is_err());
    }

    #[test]
    fn nothing_is_invented_past_the_data() {
        let c = fixture("autoeq-oratory1990-hd650.csv");
        assert_eq!(c.at(10.0), None);
        assert_eq!(c.at(20_500.0), None);
        assert!(c.at(1000.0).is_some());
    }

    #[test]
    fn bass_knob_reproduces_autoeqs_bass_boost() {
        // AutoEq's "Harman over-ear 2018" = its "without bass" + a 6 dB low
        // shelf at 105 Hz, Q 0.7. Our bass knob must give the same curve.
        let without = fixture("autoeq-harman-over-ear-2018-without-bass.csv");
        let with = fixture("autoeq-harman-over-ear-2018.csv");
        let ours = TargetKnobs {
            bass_db: 6.0,
            ..Default::default()
        }
        .apply(&without);
        let worst = ours
            .freqs
            .iter()
            .zip(&ours.db)
            .map(|(&f, v)| (v - with.at(f).unwrap()).abs())
            .fold(0.0, f64::max);
        // AutoEq's shelf is the same RBJ formula (peq.py LowShelf), at any
        // sample rate; its two files themselves differ from it by up to
        // 0.046 dB near 170 Hz (checked 2026-11-04), so that's the tolerance.
        assert!(worst < 0.05, "worst difference {worst} dB");
    }

    #[test]
    fn knobs_at_zero_change_nothing_and_tilt_pivots_mid_band() {
        assert_eq!(TargetKnobs::default().db_at(3000.0), 0.0);
        let tilt = TargetKnobs {
            tilt_db_per_oct: -1.0,
            ..Default::default()
        };
        // AutoEq: 0 at 20·√1000 Hz, ±1 dB per octave from there.
        let c = 20.0 * 1000f64.sqrt();
        assert!(tilt.db_at(c).abs() < 1e-9);
        assert!((tilt.db_at(2.0 * c) + 1.0).abs() < 1e-9);
        assert!((tilt.db_at(c / 4.0) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn anchor_puts_the_500_hz_band_at_zero() {
        let c = fixture("autoeq-oratory1990-hd650.csv").shifted(7.0);
        let off = anchor_offset(&c).unwrap();
        let k = 2f64.powf(1.0 / 6.0);
        assert!(
            c.shifted(off)
                .band_mean(500.0 / k, 500.0 * k)
                .unwrap()
                .abs()
                < 1e-9
        );
    }

    #[test]
    fn fit_moves_the_target_onto_the_measurement() {
        let meas = fixture("autoeq-oratory1990-hd650.csv");
        let target = meas.shifted(-3.0);
        assert!((fit_offset(&target, &meas, false).unwrap() - 3.0).abs() < 1e-9);
        // A sharp resonance shifts the mean but barely the median.
        let mut spiky = meas.clone();
        for (f, v) in spiky.freqs.iter().zip(spiky.db.iter_mut()) {
            if (6000.0..7000.0).contains(f) {
                *v += 15.0;
            }
        }
        let mean = fit_offset(&target, &spiky, false).unwrap();
        let median = fit_offset(&target, &spiky, true).unwrap();
        // 6–7 kHz is ~3% of the band's octaves: +15 dB there moves the mean ~0.49 dB.
        assert!(mean > 3.4, "{mean}");
        assert!((median - 3.0).abs() < 0.01, "{median}");
    }

    #[test]
    fn smoothing_keeps_flat_flat_and_widens_with_the_window() {
        let f = flat(4.0).smoothed(Smoothing::Octave(12.0));
        assert!(f.db.iter().all(|v| (v - 4.0).abs() < 1e-9));
        let mut spike = flat(0.0);
        for (f, v) in spike.freqs.iter().zip(spike.db.iter_mut()) {
            if (990.0..1010.0).contains(f) {
                *v = 10.0;
            }
        }
        let peak = |s| spike.smoothed(s).at(1000.0).unwrap();
        assert!(peak(Smoothing::Octave(24.0)) > peak(Smoothing::Octave(3.0)));
        assert!(peak(Smoothing::Erb) > peak(Smoothing::Octave(3.0)));
        assert_eq!(spike.smoothed(Smoothing::None), spike);
    }

    #[test]
    fn predicted_is_measurement_plus_eq_never_refitted() {
        let meas = Measurement {
            left: fixture("autoeq-oratory1990-hd650.csv"),
            right: None,
        };
        let freqs = [100.0, 500.0, 1000.0, 5000.0];
        let eq = [-6.0, -6.0, -2.0, 1.0]; // a preamp drop included
        let o = overlay(&OverlayInput {
            freqs: &freqs,
            measurement: Some(&meas),
            target: None,
            bounds: None,
            smoothing: Smoothing::None,
            eq_left: &eq,
            eq_right: None,
            robust_fit: false,
        });
        for ((m, p), e) in o.measurement_left.iter().zip(&o.predicted_left).zip(eq) {
            assert!((p.unwrap() - (m.unwrap() + e)).abs() < 1e-12);
        }
        assert!(o.predicted_right.is_none());
    }

    #[test]
    fn target_and_bounds_land_on_the_measurement() {
        let meas = Measurement {
            left: fixture("autoeq-oratory1990-hd650.csv"),
            right: None,
        };
        let target = fixture("autoeq-harman-over-ear-2018.csv").shifted(40.0);
        let bounds = Bounds {
            upper: flat(2.0),
            lower: flat(-2.0),
        };
        let freqs = autoeq_grid();
        let eq = vec![0.0; freqs.len()];
        let o = overlay(&OverlayInput {
            freqs: &freqs,
            measurement: Some(&meas),
            target: Some(&target),
            bounds: Some(&bounds),
            smoothing: Smoothing::Octave(12.0),
            eq_left: &eq,
            eq_right: None,
            robust_fit: false,
        });
        // The target was 40 dB away; after the fit, its mean distance from
        // the measurement over the fit band is zero.
        let placed = Curve {
            freqs: freqs.clone(),
            db: o.target.iter().map(|v| v.unwrap()).collect(),
        };
        let shown = Curve {
            freqs: freqs.clone(),
            db: o
                .measurement_left
                .iter()
                .map(|v| v.unwrap_or(0.0))
                .collect(),
        };
        assert!(fit_offset(&placed, &shown, false).unwrap().abs() < 0.05);
        let i = freqs.iter().position(|&f| f > 1000.0).unwrap();
        assert!((o.bounds_upper[i].unwrap() - o.target[i].unwrap() - 2.0).abs() < 1e-9);
        assert!((o.bounds_lower[i].unwrap() - o.target[i].unwrap() + 2.0).abs() < 1e-9);
    }

    #[test]
    fn ears_share_one_offset_so_imbalance_stays_visible() {
        let left = fixture("autoeq-oratory1990-hd650.csv");
        let right = left.shifted(-2.0); // the right ear plays 2 dB quieter
        let meas = Measurement {
            left,
            right: Some(right),
        };
        let freqs = [1000.0];
        let o = overlay(&OverlayInput {
            freqs: &freqs,
            measurement: Some(&meas),
            target: None,
            bounds: None,
            smoothing: Smoothing::None,
            eq_left: &[0.0],
            eq_right: None,
            robust_fit: false,
        });
        let gap =
            o.measurement_left[0].unwrap() - o.measurement_right.as_ref().unwrap()[0].unwrap();
        assert!((gap - 2.0).abs() < 1e-9);
    }
}
