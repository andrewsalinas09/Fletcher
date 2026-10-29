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
    /// The biquad Equalizer APO plays for `Filter: ON <kind> Fc … Gain … Q <q>`
    /// (TB-34). Not plain RBJ-by-name: APO honours a written Q on LP/HP, and
    /// reads an `LS`/`HS` line *with a Q* as a corner-frequency shelf.
    pub fn biquad(&self, fs: f64) -> Biquad {
        match apo_resolve(self.kind, self.fc_hz, self.gain_db, Some(self.q)) {
            Some(r) => Biquad::rbj(r.kind, fs, r.fc_hz, r.gain_db, r.q),
            None => Biquad::IDENTITY,
        }
    }
}

impl Biquad {
    /// Passes everything unchanged: what APO plays for a line it rejects.
    pub const IDENTITY: Biquad = Biquad {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };
}

/// Resolve a filter line the way Equalizer APO does
/// (docs/research/apo-filter-semantics.md) into the identical filter in a form
/// whose `Biquad::rbj` is exact: PK / LPQ / HPQ / BP / NO / AP / LSC / HSC with
/// the effective center frequency and Q. `q: None` means no Q was written,
/// which APO reads differently from any default Fletcher could write (a bare
/// NO is Q 30, a bare shelf is slope S 0.9). `None` = APO plays nothing (a PK
/// or AP without a Q).
pub fn apo_resolve(
    kind: FilterKind,
    fc_hz: f64,
    gain_db: f64,
    q: Option<f64>,
) -> Option<FilterSpec> {
    use FilterKind::*;
    let q = q.filter(|q| *q != 0.0); // APO: Q 0 = not given
    let spec = |kind, gain_db, q| FilterSpec {
        kind,
        fc_hz,
        gain_db,
        q,
    };
    Some(match kind {
        Peaking => spec(Peaking, gain_db, q?),
        AllPass => spec(AllPass, 0.0, q?),
        LowPass | LowPassQ => spec(LowPassQ, 0.0, q.unwrap_or(DEFAULT_Q)),
        HighPass | HighPassQ => spec(HighPassQ, 0.0, q.unwrap_or(DEFAULT_Q)),
        BandPass => spec(BandPass, 0.0, q.unwrap_or(DEFAULT_Q)),
        Notch => spec(Notch, 0.0, q.unwrap_or(30.0)),
        LowShelf | LowShelfC | HighShelf | HighShelfC => {
            let low = matches!(kind, LowShelf | LowShelfC);
            let center = matches!(kind, LowShelfC | HighShelfC);
            let (fc, q) = shelf_center_q(low, center, fc_hz, gain_db, q, None)?;
            FilterSpec {
                kind: if low { LowShelfC } else { HighShelfC },
                fc_hz: fc,
                gain_db,
                q,
            }
        }
    })
}

/// Any APO shelf spelling → the (center frequency, Q) of the identical RBJ
/// shelf. Mirrors BiQuadFilterFactory + BiQuadFilter: a slope (`LS 12dB`) wins
/// over a Q and gives S = slope/12; with neither, S = 0.9 at the center. A Q or
/// slope on a type without the trailing C (`LS`, `HS`) means a CORNER
/// frequency, moved to center by 10^(|gain|/80/S).
pub fn shelf_center_q(
    low: bool,
    center_suffix: bool,
    fc: f64,
    gain_db: f64,
    q: Option<f64>,
    slope_db: Option<f64>,
) -> Option<(f64, f64)> {
    let a = 10f64.powf(gain_db / 40.0);
    let a_sum = a + 1.0 / a;
    let (s, explicit_q, corner) = match (slope_db, q) {
        (Some(sl), _) => (sl / 12.0, None, !center_suffix),
        (None, Some(q)) => (
            1.0 / ((1.0 / (q * q) - 2.0) / a_sum + 1.0),
            Some(q),
            !center_suffix,
        ),
        (None, None) => (0.9, None, false),
    };
    let mut f = fc;
    if corner {
        let factor = 10f64.powf(gain_db.abs() / 80.0 / s);
        if low {
            f *= factor;
        } else {
            f /= factor;
        }
    }
    let q = match explicit_q {
        Some(q) => q,
        None => 1.0 / (a_sum * (1.0 / s - 1.0) + 2.0).sqrt(),
    };
    (f.is_finite() && f > 0.0 && q.is_finite() && q > 0.0).then_some((f, q))
}

/// Log-spaced frequency grid from `lo` to `hi` Hz.
pub fn log_freqs(lo: f64, hi: f64, n: usize) -> Vec<f64> {
    let (llo, lhi) = (lo.log10(), hi.log10());
    (0..n)
        .map(|i| 10f64.powf(llo + (lhi - llo) * i as f64 / (n - 1) as f64))
        .collect()
}

/// The chain's highest steady-state gain (dB) anywhere from 10 Hz to just below
/// Nyquist. Not a sparse scan (review F4: a Q 100 boost fits between 400 grid
/// points and was missed by 8.6 dB): candidates are a log grid PLUS every
/// filter's own Fc, and each local maximum is then refined by golden-section
/// search in log-frequency. What this bounds is the steady-state magnitude
/// response; it is not an inter-sample or transient true-peak bound.
pub fn response_peak_db(filters: &[FilterSpec], fs: f64) -> f64 {
    if filters.is_empty() {
        return 0.0;
    }
    let top = 0.49 * fs;
    let biquads: Vec<Biquad> = filters.iter().map(|f| f.biquad(fs)).collect();
    let resp = |f: f64| biquads.iter().map(|b| b.magnitude_db(f, fs)).sum::<f64>();
    let mut cands = log_freqs(10.0, top, 512);
    cands.extend(
        filters
            .iter()
            .map(|f| f.fc_hz)
            .filter(|f| *f > 10.0 && *f < top),
    );
    cands.sort_by(f64::total_cmp);
    cands.dedup();
    let vals: Vec<f64> = cands.iter().map(|&f| resp(f)).collect();
    let mut peak = vals.iter().copied().fold(f64::MIN, f64::max);
    const G: f64 = 0.618_033_988_749_895;
    for i in 0..vals.len() {
        let left = if i > 0 { vals[i - 1] } else { f64::MIN };
        let right = vals.get(i + 1).copied().unwrap_or(f64::MIN);
        // Strict on the left so a flat plateau is refined once, not 500 times.
        if !(vals[i] > left && vals[i] >= right) {
            continue;
        }
        let (mut a, mut b) = (
            cands[i.saturating_sub(1)].ln(),
            cands[(i + 1).min(cands.len() - 1)].ln(),
        );
        let (mut c, mut d) = (b - G * (b - a), a + G * (b - a));
        let (mut rc, mut rd) = (resp(c.exp()), resp(d.exp()));
        for _ in 0..60 {
            if rc > rd {
                b = d;
                d = c;
                rd = rc;
                c = b - G * (b - a);
                rc = resp(c.exp());
            } else {
                a = c;
                c = d;
                rc = rd;
                d = a + G * (b - a);
                rd = resp(d.exp());
            }
        }
        peak = peak.max(rc).max(rd);
    }
    peak
}

/// Preamp that keeps the summed chain at or under 0 dB (TB-06): the negative
/// of its positive peak — filter *interaction* counts, not just the largest
/// single gain. Rounded (down) to 0.1 dB.
pub fn auto_preamp_db(filters: &[FilterSpec], fs: f64) -> f64 {
    -(response_peak_db(filters, fs).max(0.0) * 10.0).ceil() / 10.0
}

/// ITU-R BS.1770 K-weighting (the LUFS pre-filter) as its two published
/// biquads at 48 kHz: a +4 dB high shelf, then the RLB high-pass.
const K_WEIGHT_48K: [Biquad; 2] = [
    Biquad {
        b0: 1.535_124_859_586_97,
        b1: -2.691_696_189_406_38,
        b2: 1.198_392_810_852_85,
        a1: -1.690_659_293_182_41,
        a2: 0.732_480_774_215_85,
    },
    Biquad {
        b0: 1.0,
        b1: -2.0,
        b2: 1.0,
        a1: -1.990_047_454_833_98,
        a2: 0.990_072_250_366_21,
    },
];

/// How much louder (dB) pink noise sounds through `filters` than through
/// nothing: the K-weighted power gain over the whole band, i.e. the LUFS change
/// a chain causes on pink noise. This is what level matching normalizes (ADR-0016).
/// It replaced the mean of the dB curve, which ignored that a dB average
/// under-counts boosts and ignores the ear's treble weighting, so EQ'd chains
/// came out up to ~1 dB louder than Flat. The coefficients are the 48 kHz set,
/// so `fs` should be 48000 (Fletcher's analysis rate).
pub fn loudness_db(filters: &[FilterSpec], fs: f64) -> f64 {
    if filters.is_empty() {
        return 0.0;
    }
    // The band LUFS itself measures: everything up to just below Nyquist (a
    // treble shelf's energy above 20 kHz counts in LUFS too), and down past
    // where the RLB high-pass removes it.
    let freqs = log_freqs(10.0, 0.49 * fs, 600);
    let resp = chain_response_db(filters, 0.0, fs, &freqs);
    let (mut num, mut den) = (0.0, 0.0);
    for (f, r) in freqs.iter().zip(&resp) {
        // Log-spaced points carry equal weight: equal power per octave = pink.
        let k = 10f64.powf(
            K_WEIGHT_48K
                .iter()
                .map(|b| b.magnitude_db(*f, fs))
                .sum::<f64>()
                / 10.0,
        );
        num += k * 10f64.powf(r / 10.0);
        den += k;
    }
    10.0 * (num / den).log10()
}

fn round_tenth(db: f64) -> f64 {
    (db * 10.0).round() / 10.0
}

/// The level plan for one chain (ADR-0016): (preamp to write, shortfall).
///
/// - `outside_preamp_db`: outside Preamp gain known to apply on this path
///   (cuts give headroom back, boosts take it away).
/// - `target_db`: where everything should land (reference + trim).
/// - `matching`: on → the preamp puts the chain's loudness at the target;
///   off → every chain gets the target preamp, so presets keep their own
///   loudness relative to Flat, which stays at the target.
/// - `range`: the written preamp's (floor, ceiling).
///
/// The clip ceiling (the chain's peak plus the outside preamp) is applied
/// LAST, so nothing (not even the floor) can put the preamp above it (review
/// F5). shortfall = target − what was achievable: positive = held below the
/// target by the ceiling; negative = the plan is infeasible (only the floor kept
/// it from going lower) and the chain plays LOUDER than intended. Callers must
/// treat negative as "cannot match". A chain with no finite loudness (e.g. a
/// high-pass at Nyquist silences everything) is reported infeasible at the
/// floor: quiet, and never a non-finite preamp.
pub fn plan_level(
    filters: &[FilterSpec],
    outside_preamp_db: f64,
    fs: f64,
    target_db: f64,
    matching: bool,
    range: (f64, f64),
) -> (f64, f64) {
    let peak = response_peak_db(filters, fs).max(0.0);
    let ceiling = (-(peak * 10.0).ceil() / 10.0 - outside_preamp_db).min(range.1);
    let want = if matching {
        target_db - loudness_db(filters, fs)
    } else {
        target_db
    };
    if !want.is_finite() || !ceiling.is_finite() {
        return (range.0, -1.0);
    }
    let preamp = want.max(range.0).min(ceiling);
    let shortfall = if matching { want - preamp } else { 0.0 };
    (round_tenth(preamp), round_tenth(shortfall))
}

/// Two plans that will be COMPARED, made equal (TB-08 v2, ADR-0016): if either
/// side's ceiling held it short, both come down by the larger shortfall, so they
/// play at the same loudness. Returns (preamp_a, preamp_b, dB below target,
/// matched). `matched` is false when equality is impossible (either plan is
/// infeasible, or lowering a side would hit the floor); the preamps are then
/// each side's own plan, and the UI must say "unmatched".
pub fn pair_levels(a: (f64, f64), b: (f64, f64), range: (f64, f64)) -> (f64, f64, f64, bool) {
    let ((pa, sa), (pb, sb)) = (a, b);
    if sa < -0.05 || sb < -0.05 {
        return (pa, pb, 0.0, false);
    }
    let below = sa.max(sb);
    let (la, lb) = (pa - (below - sa), pb - (below - sb));
    if la < range.0 || lb < range.0 {
        return (pa, pb, 0.0, false);
    }
    (round_tenth(la), round_tenth(lb), below, true)
}

/// The measured A/B correction for clip trials (ADR-0016), from the two buses'
/// real loudness on this material: returns (trim_a_db, trim_b_db) that make them
/// equal. B normally takes the whole correction; if raising B would push its
/// peak past `margin_db` below full scale, A comes down by the remainder
/// instead of leaving a residual (review F1). Silent or unmeasurable material
/// (−∞ LUFS, which would make the trim NaN) is an error, never a "match" (F9).
pub fn measured_trims(
    lufs_a: f64,
    lufs_b: f64,
    peak_b_db: f64,
    margin_db: f64,
) -> Result<(f64, f64), &'static str> {
    if !(lufs_a.is_finite() && lufs_b.is_finite() && peak_b_db.is_finite()) {
        return Err("is silent or too quiet to measure");
    }
    let want = lufs_a - lufs_b;
    if want <= 0.0 {
        return Ok((0.0, want));
    }
    let up = want.min((-peak_b_db - margin_db).max(0.0));
    Ok((-(want - up), up))
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

    fn spec(kind: FilterKind, fc_hz: f64, gain_db: f64, q: f64) -> FilterSpec {
        FilterSpec {
            kind,
            fc_hz,
            gain_db,
            q,
        }
    }

    #[test]
    fn tb34_ls_with_q_is_drawn_where_apo_plays_it() {
        // Fletcher writes "LS Fc 100 Hz Gain 6 dB Q 0.7071…". APO: corner shelf,
        // S from Q (exactly 1 at Q = 1/√2), center = 100 · 10^(6/80).
        let drawn = spec(LowShelf, 100.0, 6.0, DEFAULT_Q).biquad(FS);
        let played = Biquad::rbj(
            LowShelfC,
            FS,
            100.0 * 10f64.powf(6.0 / 80.0),
            6.0,
            DEFAULT_Q,
        );
        let hs_drawn = spec(HighShelf, 8000.0, -4.0, DEFAULT_Q).biquad(FS);
        let hs_played = Biquad::rbj(
            HighShelfC,
            FS,
            8000.0 / 10f64.powf(4.0 / 80.0),
            -4.0,
            DEFAULT_Q,
        );
        for f in log_freqs(20.0, 20000.0, 200) {
            assert!(close(
                drawn.magnitude_db(f, FS),
                played.magnitude_db(f, FS),
                1e-9
            ));
            assert!(close(
                hs_drawn.magnitude_db(f, FS),
                hs_played.magnitude_db(f, FS),
                1e-9
            ));
        }
        // The old drawing (center-frequency at 100 Hz) was visibly off.
        let old = Biquad::rbj(LowShelfC, FS, 100.0, 6.0, DEFAULT_Q);
        let worst = log_freqs(20.0, 20000.0, 400)
            .into_iter()
            .map(|f| (old.magnitude_db(f, FS) - drawn.magnitude_db(f, FS)).abs())
            .fold(0.0, f64::max);
        eprintln!("TB-34: old LS drawing off by up to {worst:.2} dB (+6 dB shelf)");
        assert!(worst > 0.5);
    }

    #[test]
    fn tb34_written_q_is_honoured_on_lp_hp() {
        // rbj(LowPass) forces 1/√2; APO plays the written Q.
        let lp = spec(LowPass, 1000.0, 0.0, 2.0).biquad(FS);
        assert!(close(
            lp.magnitude_db(1000.0, FS),
            20.0 * 2f64.log10(),
            1e-6
        ));
    }

    // ---- ADR-0016 level-plan guarantees ----
    const RANGE: (f64, f64) = (-120.0, 12.0);
    fn loud_at(f: &[FilterSpec], preamp: f64) -> f64 {
        loudness_db(f, FS) + preamp
    }
    /// Independent reference for "never clips": a DENSE grid (not the
    /// implementation's own candidates), so a missed narrow peak shows up.
    fn dense_peak(f: &[FilterSpec]) -> f64 {
        chain_response_db(f, 0.0, FS, &log_freqs(10.0, 0.49 * FS, 200_000))
            .into_iter()
            .fold(f64::MIN, f64::max)
    }
    fn peak_with(f: &[FilterSpec], preamp: f64, foreign: f64) -> f64 {
        dense_peak(f) + preamp + foreign
    }
    fn boost6() -> Vec<FilterSpec> {
        vec![spec(Peaking, 3000.0, 6.0, 1.0)]
    }

    #[test]
    fn level_plan_lands_on_target_when_it_can() {
        for f in [vec![], boost6(), vec![spec(Peaking, 1000.0, -6.0, 2.0)]] {
            let (p, s) = plan_level(&f, 0.0, FS, -8.0, true, RANGE);
            assert_eq!(s, 0.0);
            assert!(
                close(loud_at(&f, p), -8.0, 0.06),
                "lands at {}",
                loud_at(&f, p)
            );
        }
    }

    #[test]
    fn level_plan_never_clips_the_whole_path() {
        // Reference 0 + positive trim, with and without an outside −6 dB line.
        for foreign in [0.0, -6.0] {
            for target in [0.0, 6.0] {
                let (p, _) = plan_level(&boost6(), foreign, FS, target, true, RANGE);
                assert!(
                    peak_with(&boost6(), p, foreign) <= 0.06,
                    "clips at target {target}, foreign {foreign}"
                );
            }
        }
        // An outside −6 dB line gives exactly 6 dB of headroom back to the trim.
        let (p0, _) = plan_level(&[], 0.0, FS, 6.0, true, RANGE);
        let (p6, _) = plan_level(&[], -6.0, FS, 6.0, true, RANGE);
        assert_eq!((p0, p6), (0.0, 6.0));
    }

    #[test]
    fn matching_off_never_raises_flat_above_the_target() {
        // Review F1: "off" used to put Flat at 0 dB, +8 at the default reference.
        assert_eq!(plan_level(&[], 0.0, FS, -8.0, false, RANGE), (-8.0, 0.0));
        // A boosted preset keeps its own loudness relative to Flat (unmatched on purpose).
        let (p, _) = plan_level(&boost6(), 0.0, FS, -8.0, false, RANGE);
        assert_eq!(p, -8.0);
    }

    #[test]
    fn reported_bug_pair_plays_at_one_loudness() {
        // The user report: reference 0 dB, a +6 dB peak vs Flat. Solo, the preset
        // was ~2 dB quieter than Flat; paired, both land at the same loudness.
        let a = plan_level(&boost6(), 0.0, FS, 0.0, true, RANGE);
        let b = plan_level(&[], 0.0, FS, 0.0, true, RANGE);
        assert!(
            a.1 > 1.0,
            "the preset should hit its ceiling at reference 0"
        );
        let (pa, pb, below, matched) = pair_levels(a, b, RANGE);
        assert!(matched);
        assert!(close(loud_at(&boost6(), pa), loud_at(&[], pb), 0.12));
        assert!(
            close(below, a.1, 1e-9) && pb < 0.0,
            "Flat comes down with it"
        );
    }

    #[test]
    fn peak_search_finds_narrow_boosts_between_grid_points() {
        // Review F4: a Q 100 boost centred between two old grid points.
        let grid = log_freqs(20.0, 20000.0, 400);
        for (fc, q) in [
            ((grid[210] * grid[211]).sqrt(), 100.0),
            (765.1327, 100.0),
            (14_321.0, 60.0),
        ] {
            let chain = [spec(Peaking, fc, 12.0, q)];
            let found = response_peak_db(&chain, FS);
            let truth = dense_peak(&chain).max(chain_response_db(&chain, 0.0, FS, &[fc])[0]);
            assert!(
                found >= truth - 0.01,
                "fc {fc}: found {found:.3} vs true {truth:.3}"
            );
            let p = auto_preamp_db(&chain, FS);
            assert!(
                truth + p <= 0.01,
                "fc {fc}: still {:.3} dB over after preamp",
                truth + p
            );
        }
    }

    #[test]
    fn infeasible_plans_never_exceed_the_ceiling_and_never_claim_a_match() {
        // Review F5: three coincident +30 dB boosts. With a reachable floor the
        // old code clamped UP past the ceiling and reported a perfect match.
        let chain = vec![spec(Peaking, 1000.0, 30.0, 1.0); 3];
        let tight = (-80.0, 12.0);
        let a = plan_level(&chain, 0.0, FS, -50.0, true, tight);
        assert!(
            a.0 <= auto_preamp_db(&chain, FS) + 1e-9,
            "above the ceiling"
        );
        assert!(
            a.1 < 0.0,
            "infeasible must be reported as negative shortfall"
        );
        let (_, _, _, matched) =
            pair_levels(a, plan_level(&[], 0.0, FS, -50.0, true, tight), tight);
        assert!(!matched, "an impossible match must say so");
        // No finite floor covers every accepted chain (±30 dB per filter, any
        // count), so the floor is NOT the safeguard; the ordering is. Even at the
        // app's −120 floor this chain is infeasible, and that's reported, not hidden.
        let real = plan_level(&chain, 0.0, FS, -50.0, true, RANGE);
        assert!(real.0 <= auto_preamp_db(&chain, FS) + 1e-9);
        if real.1 < 0.0 {
            assert!(!pair_levels(real, plan_level(&[], 0.0, FS, -50.0, true, RANGE), RANGE).3);
        }
    }

    #[test]
    fn a_silent_chain_is_infeasible_and_quiet_never_non_finite() {
        // A high-pass at Nyquist has all-zero coefficients: no finite loudness.
        let chain = [spec(HighPass, FS / 2.0, 0.0, 0.707)];
        let (p, s) = plan_level(&chain, 0.0, FS, -8.0, true, RANGE);
        assert!(p.is_finite() && s.is_finite());
        assert_eq!((p, s < 0.0), (RANGE.0, true));
    }

    #[test]
    fn measured_trims_leave_no_residual_and_reject_silence() {
        // Review F1: B needs +5.3 dB but has only 5.0 of headroom (0.3 margin).
        let (ta, tb) = measured_trims(-10.0, -15.2987, -5.3009, 0.3).unwrap();
        assert!(
            close(-10.0 + ta, -15.2987 + tb, 1e-9),
            "residual {}",
            (-10.0 + ta) - (-15.2987 + tb)
        );
        assert!(ta < 0.0 && close(tb, 5.0009, 1e-9));
        // B louder: B simply comes down.
        assert_eq!(
            measured_trims(-12.0, -10.0, -1.0, 0.3).unwrap(),
            (0.0, -2.0)
        );
        // Review F9: silence is −∞ LUFS; never a NaN trim.
        assert!(
            measured_trims(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY, 0.3).is_err()
        );
    }

    #[test]
    fn tb34_unwritten_q_resolves_like_apo() {
        assert_eq!(apo_resolve(Notch, 800.0, 0.0, None).unwrap().q, 30.0);
        assert!(
            apo_resolve(Peaking, 800.0, 3.0, None).is_none(),
            "APO drops a Q-less PK"
        );
        // Bare shelf: slope S 0.9 at the center frequency, no corner shift.
        let ls = apo_resolve(LowShelf, 300.0, 5.0, None).unwrap();
        assert_eq!((ls.kind, ls.fc_hz), (LowShelfC, 300.0));
        let a = 10f64.powf(5.0 / 40.0);
        assert!(close(
            ls.q,
            1.0 / ((a + 1.0 / a) * (1.0 / 0.9 - 1.0) + 2.0).sqrt(),
            1e-12
        ));
        // Resolving is idempotent: a resolved spec biquads to itself.
        let again = ls.biquad(FS);
        let direct = Biquad::rbj(LowShelfC, FS, ls.fc_hz, 5.0, ls.q);
        assert!(close(
            again.magnitude_db(300.0, FS),
            direct.magnitude_db(300.0, FS),
            1e-12
        ));
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
