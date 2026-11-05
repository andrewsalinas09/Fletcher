//! Fitting a headphone to a target: AutoEq's method, ported (ADR-0021 step 6).
//!
//! AutoEq (https://github.com/jaakkopasanen/AutoEq, MIT, © 2018–2022 Jaakko
//! Pasanen) is the reference: its `FrequencyResponse.process` + `equalize`
//! and its `PEQ` optimizer, with its defaults (`constants.py`). The steps:
//!
//! 1. Measurement and target on AutoEq's grid, each set to 0 dB at 1 kHz, then
//!    the target moved so the error averages 0 over 100 Hz–10 kHz;
//!    error = measurement − target.
//! 2. The error smoothed (Savitzky–Golay, order 2): 1/12 octave, blending to
//!    2 octaves from 6 to 8 kHz, so treble peakiness (fit-dependent) isn't
//!    chased.
//! 3. Equalization = −smoothed error, its slopes limited to 18 dB/octave left
//!    to right and right to left except around dips deeper than their
//!    neighbors, boosts capped at +6 dB, smoothed again at 1/5 octave.
//! 4. Filters fitted to it on a ×1.02 grid, in AutoEq's default layout: a
//!    105 Hz low shelf plus 4 peaks (to 10 kHz), then a 10 kHz high shelf
//!    plus 4 peaks on what's left. Loss = RMS error (above 10 kHz only the
//!    average level counts) plus AutoEq's penalty on peaks steeper than
//!    18 dB/octave.
//!
//! One difference: AutoEq minimizes with scipy's SLSQP and stops early when
//! the loss stops moving; here a bounded quasi-Newton (BFGS, projected onto
//! the bounds) runs to convergence. Same loss, same bounds, same start.

use crate::config::FilterKind;
use crate::dsp::FilterSpec;
use crate::response::{Curve, autoeq_grid};

/// AutoEq's `DEFAULT_MAX_GAIN`: the most any frequency is boosted (dB).
pub const MAX_GAIN_DB: f64 = 6.0;
/// AutoEq's `DEFAULT_MAX_SLOPE` (dB/octave).
pub const MAX_SLOPE: f64 = 18.0;
const WINDOW_OCT: f64 = 1.0 / 12.0;
const TREBLE_WINDOW_OCT: f64 = 2.0;
const TREBLE_LO_HZ: f64 = 6000.0;
const TREBLE_HI_HZ: f64 = 8000.0;
const FINAL_WINDOW_OCT: f64 = 1.0 / 5.0;
/// AutoEq's `DEFAULT_FS`: the rate filter responses are computed at.
const FS: f64 = 44100.0;
const PEQ_STEP: f64 = 1.02;

/// A fit: the filters, and the curve they were fitted to.
#[derive(Debug, Clone)]
pub struct Fit {
    pub filters: Vec<FilterSpec>,
    /// The equalization AutoEq's steps 1–3 ask for, on AutoEq's grid.
    pub equalization: Curve,
}

/// Fit filters that move `measurement` to `target` (both any grid; values
/// outside their range are held at the nearest end).
pub fn fit(measurement: &Curve, target: &Curve) -> Result<Fit, String> {
    let s = stages(measurement, target)?;
    let (f, eq) = (s.f, s.equalization);
    let equalization = Curve {
        freqs: f.clone(),
        db: eq,
    };

    // The filter fit runs on AutoEq's coarser optimization grid.
    let mut pf = Vec::new();
    let mut x = 20.0_f64;
    while x <= 20000.0 {
        pf.push(x);
        x *= PEQ_STEP;
    }
    let mut remaining: Vec<f64> = pf.iter().map(|&x| clamp_at(&equalization, x)).collect();
    let mut filters = Vec::new();
    for (shelf, shelf_fc, max_f) in [
        (FilterKind::LowShelfC, 105.0, 10000.0),
        (FilterKind::HighShelfC, 10000.0, 20000.0),
    ] {
        let stage = Stage::new(&pf, &remaining, shelf, shelf_fc, max_f);
        let fitted = stage.optimize();
        let fr = stage.total_fr(&fitted);
        for (r, v) in remaining.iter_mut().zip(&fr) {
            *r -= v;
        }
        filters.extend(stage.specs(&fitted));
    }
    filters.sort_by(|a, b| a.fc_hz.total_cmp(&b.fc_hz));
    Ok(Fit {
        filters,
        equalization,
    })
}

/// Steps 1–3, each kept (the tests check them against AutoEq's own result files).
#[cfg_attr(not(test), allow(dead_code))]
struct Stages {
    f: Vec<f64>,
    target: Vec<f64>,
    error: Vec<f64>,
    error_smoothed: Vec<f64>,
    equalization: Vec<f64>,
}

fn stages(measurement: &Curve, target: &Curve) -> Result<Stages, String> {
    let f = autoeq_grid();
    let meas = centered(&on_grid(measurement, &f)?, &f);
    let mut target = centered(&on_grid(target, &f)?, &f);
    let mut error: Vec<f64> = meas.iter().zip(&target).map(|(m, t)| m - t).collect();
    // AutoEq's `min_mean_error` (on in its published results): the error's
    // mean over 100 Hz–10 kHz is moved to 0, so one narrow peak or dip at
    // 1 kHz doesn't bias the whole EQ.
    let band: Vec<f64> = f
        .iter()
        .zip(&error)
        .filter(|(x, _)| (100.0..=10000.0).contains(*x))
        .map(|(_, e)| *e)
        .collect();
    let delta = band.iter().sum::<f64>() / band.len() as f64;
    for (e, t) in error.iter_mut().zip(target.iter_mut()) {
        *e -= delta;
        *t += delta;
    }
    let error_smoothed = smoothen(&f, &error, WINDOW_OCT, TREBLE_WINDOW_OCT);
    let equalization = equalize(&f, &error_smoothed);
    Ok(Stages {
        f,
        target,
        error,
        error_smoothed,
        equalization,
    })
}

fn on_grid(c: &Curve, f: &[f64]) -> Result<Vec<f64>, String> {
    if c.freqs.is_empty() {
        return Err("an empty response".into());
    }
    Ok(f.iter().map(|&x| clamp_at(c, x)).collect())
}

/// The curve at `x`, held at its first or last value outside its range.
fn clamp_at(c: &Curve, x: f64) -> f64 {
    let (first, last) = (c.freqs[0], *c.freqs.last().unwrap());
    c.at(x.clamp(first, last)).unwrap_or(0.0)
}

/// AutoEq's `center()`: shifted so 1 kHz reads 0 dB.
fn centered(v: &[f64], f: &[f64]) -> Vec<f64> {
    let c = Curve {
        freqs: f.to_vec(),
        db: v.to_vec(),
    };
    let at1k = c.at(1000.0).unwrap_or(0.0);
    v.iter().map(|x| x - at1k).collect()
}

// ---------------- smoothing ----------------

/// AutoEq's `smoothing_window_size`: octaves to an odd sample count.
fn window_size(f: &[f64], octaves: f64) -> usize {
    let steps: f64 = f.windows(2).map(|w| w[1] / w[0]).sum::<f64>() / (f.len() - 1) as f64;
    let n = (2f64.powf(octaves).ln() / steps.ln()).round() as usize;
    if n.is_multiple_of(2) { n + 1 } else { n }
}

/// AutoEq's `log_f_sigmoid` with a_normal 0, a_treble 1: 0 well below `lo`,
/// 1 well above `hi`, a logistic in log-frequency between.
fn treble_weight(f: f64, lo: f64, hi: f64) -> f64 {
    let center = (hi / lo).sqrt() * lo;
    let half = hi.log10() - center.log10();
    let z = (f.log10() - center.log10()) / (half / 4.0);
    1.0 / (1.0 + (-z).exp())
}

/// AutoEq's `_smoothen`: a normal and a treble window blended by
/// `treble_weight`.
fn smoothen(f: &[f64], y: &[f64], window: f64, treble_window: f64) -> Vec<f64> {
    let normal = savgol2(y, window_size(f, window));
    let treble = savgol2(y, window_size(f, treble_window));
    f.iter()
        .enumerate()
        .map(|(i, &x)| {
            let k = treble_weight(x, TREBLE_LO_HZ, TREBLE_HI_HZ);
            normal[i] * (1.0 - k) + treble[i] * k
        })
        .collect()
}

/// scipy's `savgol_filter(y, n, 2)` (mode "interp"): each point is the value
/// of the least-squares parabola through the `n` points around it; the first
/// and last n/2 points take the parabola fitted to the first or last window.
fn savgol2(y: &[f64], n: usize) -> Vec<f64> {
    let len = y.len();
    if n < 3 || len < n {
        return y.to_vec();
    }
    let m = n / 2;
    let mf = m as f64;
    let den = (2.0 * mf - 1.0) * (2.0 * mf + 1.0) * (2.0 * mf + 3.0);
    let c: Vec<f64> = (0..n)
        .map(|j| {
            let k = j as f64 - mf;
            (3.0 * (3.0 * mf * mf + 3.0 * mf - 1.0) - 15.0 * k * k) / den
        })
        .collect();
    let mut out = vec![0.0; len];
    for i in m..len - m {
        out[i] = (0..n).map(|j| c[j] * y[i + j - m]).sum();
    }
    let head = parabola(&y[..n]);
    let tail = parabola(&y[len - n..]);
    for i in 0..m {
        out[i] = head(i as f64);
        out[len - m + i] = tail((n - m + i) as f64);
    }
    out
}

/// The least-squares parabola through `y` at x = 0, 1, 2, …
fn parabola(y: &[f64]) -> impl Fn(f64) -> f64 {
    let n = y.len() as f64;
    let mid = (n - 1.0) / 2.0;
    // Centered x keeps the normal equations well conditioned.
    let (mut s2, mut s4, mut sy, mut sxy, mut sx2y) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (i, &v) in y.iter().enumerate() {
        let x = i as f64 - mid;
        s2 += x * x;
        s4 += x * x * x * x;
        sy += v;
        sxy += x * v;
        sx2y += x * x * v;
    }
    // Odd sums vanish by symmetry: b = Σxy / Σx²; a, c from the even pair.
    let b = sxy / s2;
    let det = n * s4 - s2 * s2;
    let a = (sy * s4 - s2 * sx2y) / det;
    let c = (n * sx2y - s2 * sy) / det;
    move |x: f64| {
        let t = x - mid;
        a + b * t + c * t * t
    }
}

// ---------------- peaks (scipy.signal.find_peaks) ----------------

struct Peak {
    ix: usize,
    prominence: f64,
    width: f64,
}

/// scipy's local maxima: strictly higher than the left neighbor, then a
/// plateau (taken at its middle), then strictly lower. Ends never count.
fn local_maxima(x: &[f64]) -> Vec<usize> {
    let mut out = Vec::new();
    if x.len() < 3 {
        return out;
    }
    let mut i = 1;
    let last = x.len() - 1;
    while i < last {
        if x[i - 1] < x[i] {
            let mut ahead = i + 1;
            while ahead < last && x[ahead] == x[i] {
                ahead += 1;
            }
            if x[ahead] < x[i] {
                out.push((i + ahead - 1) / 2);
                i = ahead;
            }
        }
        i += 1;
    }
    out
}

/// Every local maximum with its prominence and its width at half
/// prominence (scipy's `peak_prominences` / `peak_widths`, no wlen).
fn peaks(x: &[f64]) -> Vec<Peak> {
    local_maxima(x)
        .into_iter()
        .map(|p| {
            let (mut left_min, mut left_base) = (x[p], p);
            let mut i = p as isize;
            while i >= 0 && x[i as usize] <= x[p] {
                if x[i as usize] < left_min {
                    left_min = x[i as usize];
                    left_base = i as usize;
                }
                i -= 1;
            }
            let (mut right_min, mut right_base) = (x[p], p);
            let mut i = p;
            while i < x.len() && x[i] <= x[p] {
                if x[i] < right_min {
                    right_min = x[i];
                    right_base = i;
                }
                i += 1;
            }
            let prominence = x[p] - left_min.max(right_min);
            let height = x[p] - prominence * 0.5;
            let mut i = p;
            while left_base < i && height < x[i] {
                i -= 1;
            }
            let mut left_ip = i as f64;
            if x[i] < height {
                left_ip += (height - x[i]) / (x[i + 1] - x[i]);
            }
            let mut i = p;
            while i < right_base && height < x[i] {
                i += 1;
            }
            let mut right_ip = i as f64;
            if x[i] < height {
                right_ip -= (height - x[i]) / (x[i - 1] - x[i]);
            }
            Peak {
                ix: p,
                prominence,
                width: right_ip - left_ip,
            }
        })
        .collect()
}

fn prominent(x: &[f64], min_prominence: f64) -> Vec<usize> {
    peaks(x)
        .into_iter()
        .filter(|p| p.prominence >= min_prominence)
        .map(|p| p.ix)
        .collect()
}

// ---------------- equalize ----------------

/// AutoEq's `equalize` with its defaults, from the smoothed error.
fn equalize(f: &[f64], smoothed: &[f64]) -> Vec<f64> {
    let y: Vec<f64> = smoothed.iter().map(|v| -v).collect();
    let peak_inds = prominent(&y, 1.0);
    let neg: Vec<f64> = y.iter().map(|v| -v).collect();
    let dip_inds = prominent(&neg, 1.0);
    if peak_inds.is_empty() && dip_inds.is_empty() {
        return y;
    }
    let mask = protection_mask(&y, &peak_inds, &dip_inds);
    let rtl_start = find_rtl_start(&y, &peak_inds, &dip_inds);
    let ltr = limited_ltr_slope(f, &y, MAX_SLOPE, 0, &peak_inds, &mask);
    let rtl = limited_rtl_slope(f, &y, MAX_SLOPE, rtl_start, &peak_inds, &mask);
    let combined: Vec<f64> = ltr
        .iter()
        .zip(&rtl)
        .map(|(a, b)| a.min(*b).min(MAX_GAIN_DB))
        .collect();
    smoothen(f, &combined, FINAL_WINDOW_OCT, FINAL_WINDOW_OCT)
}

fn argmin(v: &[f64]) -> usize {
    let mut best = 0;
    for (i, x) in v.iter().enumerate() {
        if *x < v[best] {
            best = i;
        }
    }
    best
}

/// AutoEq's `protection_mask`: around each dip deeper than both neighboring
/// dips, the slope limit doesn't apply (so a real dip isn't filled in).
fn protection_mask(y: &[f64], peak_inds: &[usize], dip_inds: &[usize]) -> Vec<bool> {
    let mut dips: Vec<usize> = dip_inds.to_vec();
    let mut levels: Vec<f64>;
    if !peak_inds.is_empty()
        && dips
            .last()
            .is_none_or(|&d| peak_inds[peak_inds.len() - 1] > d)
    {
        let lp = peak_inds[peak_inds.len() - 1];
        dips.push(argmin(&y[lp..]) + lp);
        levels = dips.iter().map(|&d| y[d]).collect();
    } else {
        // AutoEq appends index -1 and sets its level to the minimum of y.
        dips.push(y.len() - 1);
        levels = dips.iter().map(|&d| y[d]).collect();
        *levels.last_mut().unwrap() = y.iter().copied().fold(f64::INFINITY, f64::min);
    }
    let mut mask = vec![false; y.len()];
    if dips.len() < 3 {
        return mask;
    }
    for i in 1..dips.len() - 1 {
        let d = dips[i];
        let (tl, tr) = (levels[i - 1], levels[i + 1]);
        let Some(left) = (0..d).rev().find(|&j| y[j] >= tl) else {
            continue;
        };
        let Some(right) = (d..y.len()).find(|&j| y[j] >= tr) else {
            continue;
        };
        let (left, right) = (left + 1, right as isize - 1);
        if right >= left as isize {
            for m in mask.iter_mut().take(right as usize + 1).skip(left) {
                *m = true;
            }
        }
    }
    mask
}

fn find_rtl_start(y: &[f64], peak_inds: &[usize], dip_inds: &[usize]) -> usize {
    match (peak_inds.last(), dip_inds.last()) {
        (Some(&lp), ld) if ld.is_none_or(|&d| lp > d) => {
            let level = match ld {
                Some(&d) => y[d],
                None => y[0].max(y[y.len() - 1]),
            };
            (lp..y.len())
                .find(|&j| y[j] <= level)
                .unwrap_or(y.len() - 1)
        }
        (_, Some(&d)) => d,
        _ => y.len() - 1,
    }
}

/// AutoEq's `limited_ltr_slope` (no decay, no concha zone): walking left to
/// right, a rise steeper than `max_slope` is held to it, but only for a
/// stretch that contains a peak; elsewhere the curve is left alone.
fn limited_ltr_slope(
    x: &[f64],
    y: &[f64],
    max_slope: f64,
    start: usize,
    peak_inds: &[usize],
    free: &[bool],
) -> Vec<f64> {
    let mut limited: Vec<f64> = Vec::with_capacity(y.len());
    let mut clipped: Vec<bool> = Vec::with_capacity(y.len());
    let mut region_start: Option<usize> = None;
    for i in 0..y.len() {
        if i <= start {
            limited.push(y[i]);
            clipped.push(false);
            continue;
        }
        let octaves = (x[i] / x[i - 1]).log2();
        let prev = *limited.last().unwrap();
        let slope = (y[i] - prev) / octaves;
        if slope > max_slope && !free[i] {
            if !clipped.last().copied().unwrap_or(false) {
                region_start = Some(i);
            }
            clipped.push(true);
            limited.push(prev + max_slope * octaves);
        } else {
            limited.push(y[i]);
            if clipped.last().copied().unwrap_or(false) {
                let rs = region_start.take().unwrap_or(i);
                if !peak_inds.iter().any(|&p| p >= rs && p < i) {
                    // No peak in the clipped stretch: drop the limit there.
                    limited[rs..i].copy_from_slice(&y[rs..i]);
                    for c in &mut clipped[rs..i] {
                        *c = false;
                    }
                }
            }
            clipped.push(false);
        }
    }
    limited
}

/// AutoEq's `limited_rtl_slope`: the same walk on the reversed curve (the
/// frequency axis itself isn't reversed, as in AutoEq).
fn limited_rtl_slope(
    x: &[f64],
    y: &[f64],
    max_slope: f64,
    start: usize,
    peak_inds: &[usize],
    free: &[bool],
) -> Vec<f64> {
    let n = y.len();
    let ry: Vec<f64> = y.iter().rev().copied().collect();
    let rf: Vec<bool> = free.iter().rev().copied().collect();
    let rp: Vec<usize> = peak_inds.iter().map(|&p| n - 1 - p).collect();
    let mut out = limited_ltr_slope(x, &ry, max_slope, n - 1 - start, &rp, &rf);
    out.reverse();
    out
}

// ---------------- the filter fit ----------------

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Peak,
    Shelf(FilterKind),
}

/// One optimization pass: a fixed-frequency shelf (gain free) plus four
/// peaks (frequency, Q and gain free), fitted to `target` on `f`.
struct Stage<'a> {
    f: &'a [f64],
    target: Vec<f64>,
    shelf: FilterKind,
    shelf_fc: f64,
    max_f_ix: usize,
    ix10k: usize,
    lo: Vec<f64>,
    hi: Vec<f64>,
}

const PEAKS: usize = 4;
const PK_MIN_FC: f64 = 20.0;
const PK_MAX_FC: f64 = 10000.0;
const PK_MIN_Q: f64 = 0.18248;
const PK_MAX_Q: f64 = 6.0;
const MIN_GAIN: f64 = -20.0;
const MAX_GAIN: f64 = 20.0;
const SHELF_Q: f64 = 0.7;

fn nearest(f: &[f64], x: f64) -> usize {
    let d: Vec<f64> = f.iter().map(|v| (v - x).abs()).collect();
    argmin(&d)
}

fn fr(f: &[f64], kind: FilterKind, fc: f64, gain: f64, q: f64) -> Vec<f64> {
    let b = FilterSpec {
        kind,
        fc_hz: fc,
        gain_db: gain,
        q,
    }
    .biquad(FS);
    f.iter().map(|&x| b.magnitude_db(x, FS)).collect()
}

impl<'a> Stage<'a> {
    fn new(f: &'a [f64], target: &[f64], shelf: FilterKind, shelf_fc: f64, max_f: f64) -> Self {
        // Params: shelf gain, then (log10 fc, q, gain) per peak.
        let mut lo = vec![MIN_GAIN];
        let mut hi = vec![MAX_GAIN];
        for _ in 0..PEAKS {
            lo.extend([PK_MIN_FC.log10(), PK_MIN_Q, MIN_GAIN]);
            hi.extend([PK_MAX_FC.log10(), PK_MAX_Q, MAX_GAIN]);
        }
        Stage {
            f,
            target: target.to_vec(),
            shelf,
            shelf_fc,
            max_f_ix: nearest(f, max_f),
            ix10k: nearest(f, 10000.0),
            lo,
            hi,
        }
    }

    fn filters(&self, p: &[f64]) -> Vec<(Kind, f64, f64, f64)> {
        let mut out = vec![(Kind::Shelf(self.shelf), self.shelf_fc, p[0], SHELF_Q)];
        for k in 0..PEAKS {
            let b = 1 + 3 * k;
            out.push((Kind::Peak, 10f64.powf(p[b]), p[b + 2], p[b + 1]));
        }
        out
    }

    fn specs(&self, p: &[f64]) -> Vec<FilterSpec> {
        self.filters(p)
            .into_iter()
            .map(|(k, fc, gain, q)| FilterSpec {
                kind: match k {
                    Kind::Peak => FilterKind::Peaking,
                    Kind::Shelf(s) => s,
                },
                fc_hz: fc,
                gain_db: gain,
                q,
            })
            .collect()
    }

    fn one_fr(&self, k: Kind, fc: f64, gain: f64, q: f64) -> Vec<f64> {
        let kind = match k {
            Kind::Peak => FilterKind::Peaking,
            Kind::Shelf(s) => s,
        };
        fr(self.f, kind, fc, gain, q)
    }

    fn total_fr(&self, p: &[f64]) -> Vec<f64> {
        let mut sum = vec![0.0; self.f.len()];
        for (k, fc, gain, q) in self.filters(p) {
            for (s, v) in sum.iter_mut().zip(self.one_fr(k, fc, gain, q)) {
                *s += v;
            }
        }
        sum
    }

    /// AutoEq's `_optimizer_loss`.
    fn loss(&self, p: &[f64]) -> f64 {
        let mut total = vec![0.0; self.f.len()];
        let mut penalty = 0.0;
        for (k, fc, gain, q) in self.filters(p) {
            let r = self.one_fr(k, fc, gain, q);
            if k == Kind::Peak {
                // A peak steeper than ~18 dB/octave is penalized by its whole
                // response (AutoEq's `sharpness_penalty`).
                let limit = -0.09503189270199464 + 20.575128011847003 * (1.0 / q);
                let z = gain / limit - 1.0;
                let coef = 1.0 / (1.0 + (-z * 100.0).exp());
                penalty += r.iter().map(|v| (v * coef).powi(2)).sum::<f64>() / r.len() as f64;
            }
            for (t, v) in total.iter_mut().zip(r) {
                *t += v;
            }
        }
        let tail = |v: &[f64]| v[self.ix10k..].iter().sum::<f64>() / (v.len() - self.ix10k) as f64;
        let (tt, ft) = (tail(&self.target), tail(&total));
        let sse: f64 = self
            .target
            .iter()
            .zip(&total)
            .take(self.max_f_ix)
            .enumerate()
            .map(|(i, (t, v))| {
                if i >= self.ix10k {
                    (tt - ft).powi(2)
                } else {
                    (t - v).powi(2)
                }
            })
            .sum();
        (sse / self.max_f_ix as f64 + penalty).sqrt()
    }

    /// AutoEq's `_init_optimizer_params`: the shelf first (gain = the
    /// target's average weighted by a 1 dB shelf), then each peak on the
    /// biggest remaining peak or dip (width × height).
    fn init(&self) -> Vec<f64> {
        let mut rem = self.target.clone();
        let one = self.one_fr(Kind::Shelf(self.shelf), self.shelf_fc, 1.0, SHELF_Q);
        let w: f64 = one.iter().sum();
        let gain =
            (rem.iter().zip(&one).map(|(t, o)| t * o).sum::<f64>() / w).clamp(MIN_GAIN, MAX_GAIN);
        for (r, v) in
            rem.iter_mut()
                .zip(self.one_fr(Kind::Shelf(self.shelf), self.shelf_fc, gain, SHELF_Q))
        {
            *r -= v;
        }
        let mut p = vec![gain];
        let (min_ix, max_ix) = (nearest(self.f, PK_MIN_FC), nearest(self.f, PK_MAX_FC));
        let f_step = (self.f[1] / self.f[0]).log2();
        for _ in 0..PEAKS {
            let pos: Vec<f64> = rem.iter().map(|v| v.max(0.0)).collect();
            let neg: Vec<f64> = rem.iter().map(|v| (-v).max(0.0)).collect();
            let mut best: Option<(usize, f64, f64)> = None; // ix, width, height
            for (x, _) in [(&pos, 1.0), (&neg, -1.0)] {
                for pk in peaks(x) {
                    if pk.ix < min_ix || pk.ix > max_ix {
                        continue;
                    }
                    let h = x[pk.ix];
                    if best.is_none_or(|b| pk.width * h > b.1 * b.2) {
                        best = Some((pk.ix, pk.width, h));
                    }
                }
            }
            let (fc, q, gain) = match best {
                None => (self.f[(min_ix + max_ix) / 2], std::f64::consts::SQRT_2, 0.0),
                Some((ix, width, h)) => {
                    let bw = f_step * width;
                    let q =
                        (2f64.powf(bw).sqrt() / (2f64.powf(bw) - 1.0)).clamp(PK_MIN_Q, PK_MAX_Q);
                    let g = if rem[ix] > 0.0 { h } else { -h };
                    (
                        self.f[ix].clamp(PK_MIN_FC, PK_MAX_FC),
                        q,
                        g.clamp(MIN_GAIN, MAX_GAIN),
                    )
                }
            };
            for (r, v) in rem.iter_mut().zip(self.one_fr(Kind::Peak, fc, gain, q)) {
                *r -= v;
            }
            p.extend([fc.log10(), q, gain]);
        }
        p
    }

    fn optimize(&self) -> Vec<f64> {
        minimize_bounded(|p| self.loss(p), self.init(), &self.lo, &self.hi)
    }
}

/// Minimize `f` within [lo, hi]: BFGS on the free variables, each step
/// projected onto the bounds, with a backtracking (Armijo) line search and a
/// central-difference gradient.
fn minimize_bounded(f: impl Fn(&[f64]) -> f64, x0: Vec<f64>, lo: &[f64], hi: &[f64]) -> Vec<f64> {
    let n = x0.len();
    let clamp = |x: &mut [f64]| {
        for i in 0..n {
            x[i] = x[i].clamp(lo[i], hi[i]);
        }
    };
    let grad = |x: &[f64]| -> Vec<f64> {
        let mut g = vec![0.0; n];
        let mut xp = x.to_vec();
        for i in 0..n {
            let h = 1e-6 * (1.0 + x[i].abs());
            let (a, b) = ((x[i] - h).max(lo[i]), (x[i] + h).min(hi[i]));
            xp[i] = b;
            let fb = f(&xp);
            xp[i] = a;
            let fa = f(&xp);
            xp[i] = x[i];
            g[i] = (fb - fa) / (b - a);
        }
        g
    };
    let identity = |n: usize| {
        let mut m = vec![vec![0.0; n]; n];
        for (i, row) in m.iter_mut().enumerate() {
            row[i] = 1.0;
        }
        m
    };
    let mut x = x0;
    clamp(&mut x);
    let mut fx = f(&x);
    let mut g = grad(&x);
    let mut h = identity(n);
    let mut still = 0;
    for _ in 0..600 {
        // Variables held at a bound by the gradient are frozen this step.
        let free: Vec<bool> = (0..n)
            .map(|i| !((x[i] <= lo[i] && g[i] > 0.0) || (x[i] >= hi[i] && g[i] < 0.0)))
            .collect();
        let mut d: Vec<f64> = (0..n)
            .map(|i| {
                if !free[i] {
                    return 0.0;
                }
                -(0..n)
                    .filter(|&j| free[j])
                    .map(|j| h[i][j] * g[j])
                    .sum::<f64>()
            })
            .collect();
        let mut slope: f64 = d.iter().zip(&g).map(|(a, b)| a * b).sum();
        if slope >= 0.0 {
            h = identity(n);
            d = (0..n).map(|i| if free[i] { -g[i] } else { 0.0 }).collect();
            slope = d.iter().zip(&g).map(|(a, b)| a * b).sum();
            if slope >= -1e-14 {
                break;
            }
        }
        let mut t = 1.0;
        let mut next = None;
        for _ in 0..40 {
            let mut xn: Vec<f64> = x.iter().zip(&d).map(|(a, b)| a + t * b).collect();
            clamp(&mut xn);
            let fxn = f(&xn);
            let moved: f64 = xn
                .iter()
                .zip(&x)
                .zip(&g)
                .map(|((a, b), gi)| (a - b) * gi)
                .sum();
            if fxn <= fx + 1e-4 * moved.min(0.0) && fxn <= fx {
                next = Some((xn, fxn));
                break;
            }
            t *= 0.5;
        }
        let Some((xn, fxn)) = next else {
            break;
        };
        let gn = grad(&xn);
        let s: Vec<f64> = xn.iter().zip(&x).map(|(a, b)| a - b).collect();
        let y: Vec<f64> = gn.iter().zip(&g).map(|(a, b)| a - b).collect();
        let sy: f64 = s.iter().zip(&y).map(|(a, b)| a * b).sum();
        if sy > 1e-12 {
            // h ← (I − ρ s yᵀ) h (I − ρ y sᵀ) + ρ s sᵀ
            let rho = 1.0 / sy;
            let hy: Vec<f64> = (0..n)
                .map(|i| (0..n).map(|j| h[i][j] * y[j]).sum())
                .collect();
            let yhy: f64 = y.iter().zip(&hy).map(|(a, b)| a * b).sum();
            for i in 0..n {
                for j in 0..n {
                    h[i][j] += -rho * (hy[i] * s[j] + s[i] * hy[j])
                        + (rho * rho * yhy + rho) * s[i] * s[j];
                }
            }
        }
        let gained = fx - fxn;
        x = xn;
        g = gn;
        fx = fxn;
        if gained < 1e-10 * (1.0 + fx) {
            still += 1;
            if still >= 5 {
                break;
            }
        } else {
            still = 0;
        }
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::chain_response_db;

    fn fixture(name: &str) -> Curve {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        Curve::parse(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    /// Steps 1–3 against the columns of AutoEq's own result file for this
    /// pair (it prints two decimals).
    #[test]
    fn stages_match_autoeq_result_file() {
        let text = std::fs::read_to_string(format!(
            "{}/tests/fixtures/autoeq-result-oratory1990-hd650.csv",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let rows: Vec<Vec<f64>> = text
            .lines()
            .skip(1)
            .map(|l| {
                l.split(',')
                    .map(|v| v.parse().unwrap_or(f64::NAN))
                    .collect()
            })
            .collect();
        let col = |i: usize| rows.iter().map(|r| r[i]).collect::<Vec<f64>>();
        let s = stages(
            &fixture("autoeq-oratory1990-hd650.csv"),
            &fixture("autoeq-harman-over-ear-2018.csv"),
        )
        .unwrap();
        assert_eq!(s.f.len(), rows.len());
        let worst = |ours: &[f64], theirs: &[f64]| {
            ours.iter()
                .zip(theirs)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0, f64::max)
        };
        // frequency,raw,smoothed,error,error_smoothed,equalization,…,target
        let (t, e, es, eq) = (
            worst(&s.target, &col(10)),
            worst(&s.error, &col(3)),
            worst(&s.error_smoothed, &col(4)),
            worst(&s.equalization, &col(5)),
        );
        eprintln!("worst: target {t:.3}, error {e:.3}, smoothed {es:.3}, equalization {eq:.3}");
        assert!(t < 0.08 && e < 0.08 && es < 0.08 && eq < 0.08);
    }

    #[test]
    fn savgol_keeps_a_parabola_and_smooths_noise() {
        let y: Vec<f64> = (0..50)
            .map(|i| 0.3 * (i as f64).powi(2) - 2.0 * i as f64 + 1.0)
            .collect();
        let s = savgol2(&y, 9);
        for (a, b) in y.iter().zip(&s) {
            assert!((a - b).abs() < 1e-9, "{a} vs {b}");
        }
        let zig: Vec<f64> = (0..50)
            .map(|i| if i % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        let s = savgol2(&zig, 9);
        assert!(s[25].abs() < 0.2);
    }

    #[test]
    fn peaks_match_scipy_on_a_small_case() {
        // scipy: find_peaks([0,2,1,3,3,1,0]) → [1, 3]; prominences [1, 3].
        let x = [0.0, 2.0, 1.0, 3.0, 3.0, 1.0, 0.0];
        let p = peaks(&x);
        assert_eq!(p.iter().map(|p| p.ix).collect::<Vec<_>>(), vec![1, 3]);
        assert!((p[0].prominence - 1.0).abs() < 1e-12);
        assert!((p[1].prominence - 3.0).abs() < 1e-12);
    }

    /// The whole fit against AutoEq's own published result for this pair
    /// (results/oratory1990/over-ear/Sennheiser HD 650, Harman over-ear
    /// 2018): the two EQs' responses agree within a fraction of a dB where
    /// it counts (to 10 kHz; above, only the average level is fitted).
    #[test]
    fn hd650_to_harman_matches_autoeq() {
        let meas = fixture("autoeq-oratory1990-hd650.csv");
        let target = fixture("autoeq-harman-over-ear-2018.csv");
        let fit = fit(&meas, &target).unwrap();
        assert_eq!(fit.filters.len(), 10);
        let autoeq = [
            (FilterKind::LowShelfC, 105.0, 6.4, 0.70),
            (FilterKind::Peaking, 8800.0, 5.1, 1.42),
            (FilterKind::Peaking, 118.0, -3.1, 0.50),
            (FilterKind::Peaking, 37.0, 0.7, 3.96),
            (FilterKind::Peaking, 3169.0, -1.7, 3.89),
            (FilterKind::HighShelfC, 10000.0, -2.1, 0.70),
            (FilterKind::Peaking, 1227.0, -1.2, 2.53),
            (FilterKind::Peaking, 2055.0, 1.2, 3.23),
            (FilterKind::Peaking, 587.0, 0.4, 1.19),
            (FilterKind::Peaking, 5332.0, -1.1, 5.75),
        ]
        .map(|(kind, fc_hz, gain_db, q)| FilterSpec {
            kind,
            fc_hz,
            gain_db,
            q,
        });
        let freqs: Vec<f64> = autoeq_grid()
            .into_iter()
            .filter(|&f| f <= 10000.0)
            .collect();
        let ours = chain_response_db(&fit.filters, 0.0, 48000.0, &freqs);
        let theirs = chain_response_db(&autoeq, 0.0, 48000.0, &freqs);
        let diff: Vec<f64> = ours.iter().zip(&theirs).map(|(a, b)| a - b).collect();
        let rms = (diff.iter().map(|d| d * d).sum::<f64>() / diff.len() as f64).sqrt();
        let worst = diff.iter().map(|d| d.abs()).fold(0.0, f64::max);
        eprintln!("fit vs AutoEq: rms {rms:.3} dB, worst {worst:.3} dB");
        for s in &fit.filters {
            eprintln!(
                "  {:?} {:.0} Hz {:+.1} dB Q {:.2}",
                s.kind, s.fc_hz, s.gain_db, s.q
            );
        }
        // Measured 2026-11-04: rms 0.063 dB, worst 0.27 dB.
        assert!(rms < 0.15, "rms {rms}");
        assert!(worst < 0.5, "worst {worst}");
    }
}
