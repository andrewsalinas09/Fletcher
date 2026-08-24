//! Test statistics for the Listening Lab.
//!
//! Discrimination (ABX) verdicts use the exact one-sided binomial test:
//! the probability of scoring at least `correct` out of `trials` by pure
//! guessing (p = 0.5). Preference verdicts use the exact TWO-sided test
//! (no correct answer exists — TB-15). Adaptive ABX uses Wald's SPRT for
//! honest sequential stopping. No approximations — trial counts are small.

/// P(X ≥ correct) for X ~ Binomial(trials, 0.5). Returns 1.0 for correct = 0.
pub fn binomial_p_one_sided(correct: u32, trials: u32) -> f64 {
    if trials == 0 {
        return 1.0;
    }
    let correct = correct.min(trials);
    // Sum C(n,k)/2^n for k = correct..=n, building C(n,k) incrementally.
    let n = trials as f64;
    let mut coeff = 1.0f64; // C(n, 0)
    let mut cumulative = 0.0f64;
    for k in 0..=trials {
        if k >= correct {
            cumulative += coeff;
        }
        coeff = coeff * (n - k as f64) / (k as f64 + 1.0);
    }
    (cumulative / 2f64.powi(trials as i32)).min(1.0)
}

/// Exact two-sided binomial test against p = 0.5 — the preference protocol.
/// The null is symmetric, so P(|X − n/2| ≥ |k − n/2|) = 2·P(X ≥ max(k, n−k)),
/// clamped to 1; exactly 1.0 at a perfect split (no evidence either way).
pub fn binomial_p_two_sided(k: u32, n: u32) -> f64 {
    if n == 0 {
        return 1.0;
    }
    let k = k.min(n);
    let hi = k.max(n - k);
    if 2 * hi == n {
        1.0
    } else {
        (2.0 * binomial_p_one_sided(hi, n)).min(1.0)
    }
}

/// The state of a running sequential (SPRT) test after each vote.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SprtDecision {
    /// Neither bound crossed — keep testing.
    Continue,
    /// Difference heard (H1 accepted at the calibrated α).
    AcceptH1,
    /// No difference detectable at this sensitivity (H0 accepted at β).
    AcceptH0,
}

/// Wald's SPRT for Bernoulli trials: H0 p = 0.5 (guessing) vs H1 p = p1.
/// LLR = k·ln(p1/0.5) + (n−k)·ln((1−p1)/0.5); H1 at LLR ≥ ln((1−β)/α),
/// H0 at LLR ≤ ln(β/(1−α)). The caller declares p1/α/β BEFORE the first
/// vote and records them — the stop rule is part of the test's provenance,
/// never chosen after seeing data.
pub fn sprt_step(correct: u32, trials: u32, p1: f64, alpha: f64, beta: f64) -> SprtDecision {
    if trials == 0 {
        return SprtDecision::Continue;
    }
    let correct = correct.min(trials);
    let llr =
        correct as f64 * (p1 / 0.5).ln() + (trials - correct) as f64 * ((1.0 - p1) / 0.5).ln();
    let upper = ((1.0 - beta) / alpha).ln();
    let lower = (beta / (1.0 - alpha)).ln();
    if llr >= upper {
        SprtDecision::AcceptH1
    } else if llr <= lower {
        SprtDecision::AcceptH0
    } else {
        SprtDecision::Continue
    }
}

/// The decision thresholds at a given trial count, for provenance/plots:
/// (largest k that accepts H0 at n, smallest k that accepts H1 at n) —
/// `None` where that verdict is unreachable at this n.
pub fn sprt_bounds_at(n: u32, p1: f64, alpha: f64, beta: f64) -> (Option<u32>, Option<u32>) {
    let mut h0 = None;
    let mut h1 = None;
    for k in 0..=n {
        match sprt_step(k, n, p1, alpha, beta) {
            SprtDecision::AcceptH0 => h0 = Some(k),
            SprtDecision::AcceptH1 => {
                if h1.is_none() {
                    h1 = Some(k);
                }
            }
            SprtDecision::Continue => {}
        }
    }
    (h0, h1)
}

// ---------------- the hearing profile's adaptive staircase (Q-19) ----------------

/// Why a staircase stopped.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StaircaseEnd {
    /// Reached the required count of minimum-step reversals.
    Converged,
    /// Hit the trial cap with enough min-step reversals to estimate (flagged).
    CappedOut,
    /// Hit the trial cap without a usable estimate.
    Unconverged,
    /// Pushed against the high clamp twice in a row — PSE censored above.
    RailedHigh,
    /// Pushed against the low clamp twice in a row — PSE censored below.
    RailedLow,
}

#[derive(Clone, Copy, Debug)]
pub struct StaircaseConfig {
    pub start_db: f64,
    pub initial_step_db: f64,
    pub min_step_db: f64,
    /// Halve the step after this many reversals at the current step size.
    pub reversals_per_halving: u32,
    /// Stop after this many reversals at the minimum step.
    pub min_step_reversals: u32,
    pub max_trials: u32,
    /// (floor, ceil) in dB — ceil already headroom-limited by the caller.
    pub clamp_db: (f64, f64),
}

/// 1-up-1-down adaptive staircase converging on the 50% point of
/// "comparison judged louder" — which IS the point of subjective equality.
/// Pure state machine: `level_db()` is what to present next; `answer()`
/// advances; replaying a journal of answers reconstructs it exactly (the
/// resume mechanism). A reversal is recorded at the currently presented
/// level whenever the direction flips.
pub struct Staircase {
    cfg: StaircaseConfig,
    level_db: f64,
    step_db: f64,
    last_dir: Option<f64>,
    /// (level_db, step_db in effect when the reversal was recorded)
    reversals: Vec<(f64, f64)>,
    reversals_at_step: u32,
    trials: u32,
    consecutive_clamped: u32,
    end: Option<StaircaseEnd>,
    railed_bound: Option<f64>,
}

impl Staircase {
    pub fn new(cfg: StaircaseConfig) -> Self {
        Staircase {
            level_db: cfg.start_db.clamp(cfg.clamp_db.0, cfg.clamp_db.1),
            step_db: cfg.initial_step_db,
            cfg,
            last_dir: None,
            reversals: Vec::new(),
            reversals_at_step: 0,
            trials: 0,
            consecutive_clamped: 0,
            end: None,
            railed_bound: None,
        }
    }

    /// The comparison level (dB re anchor, normalized axis) to present next.
    pub fn level_db(&self) -> f64 {
        self.level_db
    }

    pub fn done(&self) -> Option<StaircaseEnd> {
        self.end
    }

    pub fn trials(&self) -> u32 {
        self.trials
    }

    pub fn reversals(&self) -> &[(f64, f64)] {
        &self.reversals
    }

    fn min_step_reversal_levels(&self) -> Vec<f64> {
        self.reversals
            .iter()
            .filter(|(_, s)| *s <= self.cfg.min_step_db)
            .map(|(l, _)| *l)
            .collect()
    }

    /// Mean of the min-step reversals. None until done, and None for
    /// railed/unconverged ends.
    pub fn estimate_db(&self) -> Option<f64> {
        match self.end? {
            StaircaseEnd::Converged | StaircaseEnd::CappedOut => {
                let levels = self.min_step_reversal_levels();
                if levels.is_empty() {
                    None
                } else {
                    Some(levels.iter().sum::<f64>() / levels.len() as f64)
                }
            }
            _ => None,
        }
    }

    /// The censoring bound for railed ends (the clamp that was hit).
    pub fn bound_db(&self) -> Option<f64> {
        self.railed_bound
    }

    /// Feed one trial's outcome. No-op once done (defensive; journaled anyway).
    pub fn answer(&mut self, comparison_judged_louder: bool) {
        if self.end.is_some() {
            return;
        }
        self.trials += 1;
        // Comparison louder → turn it down; anchor louder → turn it up.
        let dir = if comparison_judged_louder { -1.0 } else { 1.0 };
        if let Some(last) = self.last_dir
            && last != dir
        {
            // Reversal at the currently presented level.
            self.reversals.push((self.level_db, self.step_db));
            self.reversals_at_step += 1;
            if self.step_db <= self.cfg.min_step_db {
                let n = self
                    .reversals
                    .iter()
                    .filter(|(_, s)| *s <= self.cfg.min_step_db)
                    .count() as u32;
                if n >= self.cfg.min_step_reversals {
                    self.end = Some(StaircaseEnd::Converged);
                    return;
                }
            } else if self.reversals_at_step >= self.cfg.reversals_per_halving {
                self.step_db = (self.step_db / 2.0).max(self.cfg.min_step_db);
                self.reversals_at_step = 0;
            }
        }
        self.last_dir = Some(dir);
        let want = self.level_db + dir * self.step_db;
        let clamped = want.clamp(self.cfg.clamp_db.0, self.cfg.clamp_db.1);
        if (want - clamped).abs() > 1e-9 {
            self.consecutive_clamped += 1;
            if self.consecutive_clamped >= 2 {
                self.end = Some(if want > clamped {
                    StaircaseEnd::RailedHigh
                } else {
                    StaircaseEnd::RailedLow
                });
                self.railed_bound = Some(clamped);
                return;
            }
        } else {
            self.consecutive_clamped = 0;
        }
        self.level_db = clamped;
        if self.trials >= self.cfg.max_trials {
            let n = self.min_step_reversal_levels().len() as u32;
            self.end = Some(if n >= self.cfg.min_step_reversals.min(4) {
                StaircaseEnd::CappedOut
            } else {
                StaircaseEnd::Unconverged
            });
        }
    }
}

/// Combined per-band estimate from the band's two bracketing staircases.
#[derive(Clone, Copy, Debug)]
pub struct PseEstimate {
    pub pse_db: f64,
    /// max(|Δ|/2, s_pooled/√(n/2), min_step/2) — a display bound that never
    /// understates (drift shows in the disagreement term; consecutive
    /// reversals are negatively autocorrelated, hence the effective n/2).
    pub uncertainty_db: f64,
    pub disagreement_db: f64,
}

/// Combine two finished staircases; None unless both produced estimates
/// (railed/unconverged bands report from their staircase ends directly).
pub fn combine_pse(a: &Staircase, b: &Staircase) -> Option<PseEstimate> {
    let (ea, eb) = (a.estimate_db()?, b.estimate_db()?);
    let pse = (ea + eb) / 2.0;
    let disagreement = (ea - eb).abs();
    let (la, lb) = (a.min_step_reversal_levels(), b.min_step_reversal_levels());
    let n = (la.len() + lb.len()) as f64;
    let ss: f64 = la.iter().map(|x| (x - ea) * (x - ea)).sum::<f64>()
        + lb.iter().map(|x| (x - eb) * (x - eb)).sum::<f64>();
    let s_pooled = if n > 2.0 {
        (ss / (n - 2.0)).sqrt()
    } else {
        0.0
    };
    let u = (disagreement / 2.0)
        .max(s_pooled / (n / 2.0).sqrt())
        .max(a.cfg.min_step_db / 2.0);
    Some(PseEstimate {
        pse_db: pse,
        uncertainty_db: u,
        disagreement_db: disagreement,
    })
}

/// A tiny deterministic RNG (xorshift64*) — no external dependency, good
/// enough for trial assignment. Seed from the caller (e.g. wall clock).
pub struct Xorshift(u64);

impl Xorshift {
    pub fn new(seed: u64) -> Self {
        Xorshift(seed.max(1))
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }

    pub fn next_bool(&mut self) -> bool {
        (self.next_u64() >> 63) == 1
    }

    /// Uniform in [-1, 1) — the signal generator's white-noise source.
    pub fn next_pm1(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 / (1u64 << 52) as f64) - 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() < tol
    }

    #[test]
    fn known_binomial_values() {
        // 8/8: 1/256
        assert!(close(binomial_p_one_sided(8, 8), 1.0 / 256.0, 1e-12));
        // 14/16: (C(16,14)+C(16,15)+C(16,16))/2^16 = 137/65536
        assert!(close(binomial_p_one_sided(14, 16), 137.0 / 65536.0, 1e-12));
        // 9/16 ≈ 0.4018 — indistinguishable from guessing
        assert!(close(binomial_p_one_sided(9, 16), 0.401810, 1e-5));
        // Degenerate cases
        assert_eq!(binomial_p_one_sided(0, 16), 1.0);
        assert_eq!(binomial_p_one_sided(0, 0), 1.0);
        assert!(close(binomial_p_one_sided(17, 16), 1.0 / 65536.0, 1e-12));
    }

    #[test]
    fn known_two_sided_values() {
        // 12/16 (or symmetric 4/16): 2·(1820+560+120+16+1)/65536 = 2517/32768
        assert!(close(binomial_p_two_sided(12, 16), 2517.0 / 32768.0, 1e-12));
        assert!(close(binomial_p_two_sided(4, 16), 2517.0 / 32768.0, 1e-12));
        // Perfect split = no evidence either way
        assert_eq!(binomial_p_two_sided(8, 16), 1.0);
        // 9/10: 2·(10+1)/1024 = 22/1024
        assert!(close(binomial_p_two_sided(9, 10), 22.0 / 1024.0, 1e-12));
        // 10/10: 2/1024
        assert!(close(binomial_p_two_sided(10, 10), 2.0 / 1024.0, 1e-12));
        assert_eq!(binomial_p_two_sided(0, 0), 1.0);
    }

    #[test]
    fn sprt_known_decisions() {
        use SprtDecision::*;
        let s = |k, n| sprt_step(k, n, 0.75, 0.05, 0.05);
        // Bounds are ±ln 19 ≈ ±2.944439
        assert_eq!(s(8, 8), AcceptH1); // LLR 3.2437
        assert_eq!(s(7, 7), Continue); // 2.8383
        assert_eq!(s(9, 10), AcceptH1); // 2.9560, just over
        assert_eq!(s(8, 10), Continue); // 1.8574
        assert_eq!(s(3, 10), AcceptH0); // −3.6356
        assert_eq!(s(4, 10), Continue); // −2.5370
        assert_eq!(s(0, 5), AcceptH0); // −3.4657
        assert_eq!(s(0, 4), Continue); // −2.7726
        assert_eq!(s(13, 16), AcceptH1); // 3.1916
        assert_eq!(s(12, 16), Continue); // 2.0930
        assert_eq!(s(0, 0), Continue);
    }

    #[test]
    fn sprt_bounds_shapes() {
        assert_eq!(sprt_bounds_at(10, 0.75, 0.05, 0.05), (Some(3), Some(9)));
        // At n = 5 a "difference" verdict is unreachable (5/5 LLR ≈ 2.03 < bound)
        // but "no difference" is (0/5).
        assert_eq!(sprt_bounds_at(5, 0.75, 0.05, 0.05), (Some(0), None));
    }

    fn cfg(start: f64) -> StaircaseConfig {
        StaircaseConfig {
            start_db: start,
            initial_step_db: 4.0,
            min_step_db: 1.0,
            reversals_per_halving: 2,
            min_step_reversals: 6,
            max_trials: 40,
            clamp_db: (-20.0, 20.0),
        }
    }

    #[test]
    fn staircase_hand_traced_convergence() {
        // Deterministic listener: comparison judged louder iff level ≥ 3.
        // The exact trace this freezes: presented levels
        // 8,4,0,4,2,4,3,2,3,2,3,2,3 — done after trial 13, estimate 2.5.
        let mut s = Staircase::new(cfg(8.0));
        let expected = [
            8.0, 4.0, 0.0, 4.0, 2.0, 4.0, 3.0, 2.0, 3.0, 2.0, 3.0, 2.0, 3.0,
        ];
        for (i, want) in expected.iter().enumerate() {
            assert!(s.done().is_none(), "ended early at trial {}", i + 1);
            assert!(
                close(s.level_db(), *want, 1e-12),
                "trial {}: presented {} expected {}",
                i + 1,
                s.level_db(),
                want
            );
            s.answer(s.level_db() >= 3.0);
        }
        assert_eq!(s.done(), Some(StaircaseEnd::Converged));
        assert_eq!(s.trials(), 13);
        assert_eq!(s.reversals().len(), 10);
        assert!(close(s.estimate_db().unwrap(), 2.5, 1e-12));
    }

    /// A stochastic listener: P(comparison louder) is logistic around a PSE.
    fn logistic_answer(rng: &mut Xorshift, level: f64, pse: f64, spread: f64) -> bool {
        let p = 1.0 / (1.0 + (-(level - pse) / spread).exp());
        let u = (rng.next_pm1() + 1.0) / 2.0;
        u < p
    }

    #[test]
    fn staircase_converges_on_a_noisy_listener() {
        let mut rng = Xorshift::new(11);
        let mut s = Staircase::new(cfg(0.0));
        while s.done().is_none() {
            let level = s.level_db();
            s.answer(logistic_answer(&mut rng, level, -4.2, 1.5));
        }
        assert_eq!(s.done(), Some(StaircaseEnd::Converged));
        let est = s.estimate_db().unwrap();
        assert!(
            (est - (-4.2)).abs() <= 1.5,
            "estimate {est} too far from true PSE −4.2"
        );
    }

    #[test]
    fn staircase_rails_honestly() {
        let mut s = Staircase::new(StaircaseConfig {
            start_db: 0.0,
            clamp_db: (-6.0, 6.0),
            ..cfg(0.0)
        });
        // Listener PSE at +10: the comparison is never louder in range.
        while s.done().is_none() {
            s.answer(false);
        }
        assert_eq!(s.done(), Some(StaircaseEnd::RailedHigh));
        assert_eq!(s.estimate_db(), None);
        assert!(close(s.bound_db().unwrap(), 6.0, 1e-12));
    }

    /// A deterministic threshold listener at min step from the start: its
    /// min-step reversals alternate thresh, thresh−1, … (mean thresh − 0.5).
    fn staircase_with_threshold(start: f64, thresh: f64) -> Staircase {
        let mut s = Staircase::new(StaircaseConfig {
            start_db: start,
            initial_step_db: 1.0,
            min_step_db: 1.0,
            reversals_per_halving: 2,
            min_step_reversals: 6,
            max_trials: 200,
            clamp_db: (-20.0, 20.0),
        });
        while s.done().is_none() {
            s.answer(s.level_db() >= thresh);
        }
        s
    }

    #[test]
    fn combine_pse_exact_values() {
        // {3,2,3,2,3,2} (mean 2.5) vs {4,3,4,3,4,3} (mean 3.5):
        let a = staircase_with_threshold(1.0, 3.0);
        let b = staircase_with_threshold(2.0, 4.0);
        assert!(close(a.estimate_db().unwrap(), 2.5, 1e-12));
        assert!(close(b.estimate_db().unwrap(), 3.5, 1e-12));
        let e = combine_pse(&a, &b).unwrap();
        assert!(close(e.pse_db, 3.0, 1e-12));
        assert!(close(e.disagreement_db, 1.0, 1e-12));
        // s_pooled = √(3.0/10) ≈ 0.5477 → /√6 ≈ 0.2236; |Δ|/2 = 0.5 wins.
        assert!(close(e.uncertainty_db, 0.5, 1e-12));
        // Identical staircases: the min-step/2 floor binds.
        let c = staircase_with_threshold(1.0, 3.0);
        let e2 = combine_pse(&a, &c).unwrap();
        assert!(close(e2.disagreement_db, 0.0, 1e-12));
        assert!(close(e2.uncertainty_db, 0.5, 1e-12));
    }

    #[test]
    fn staircase_journal_replay_is_identical() {
        // Record a noisy run, then replay the journal into a fresh staircase:
        // levels and end state must match exactly — this is the resume proof.
        let mut rng = Xorshift::new(23);
        let mut live = Staircase::new(cfg(8.0));
        let mut journal: Vec<(f64, bool)> = Vec::new();
        while live.done().is_none() {
            let level = live.level_db();
            let ans = logistic_answer(&mut rng, level, 1.0, 2.0);
            journal.push((level, ans));
            live.answer(ans);
        }
        let mut replay = Staircase::new(cfg(8.0));
        for (level, ans) in &journal {
            assert!(close(replay.level_db(), *level, 1e-12));
            replay.answer(*ans);
        }
        assert_eq!(replay.done(), live.done());
        assert_eq!(replay.trials(), live.trials());
        assert_eq!(replay.estimate_db(), live.estimate_db());
    }

    #[test]
    fn rng_is_roughly_balanced_and_deterministic() {
        let mut rng = Xorshift::new(42);
        let heads = (0..10_000).filter(|_| rng.next_bool()).count();
        assert!((4_500..5_500).contains(&heads), "heads = {heads}");
        let a: Vec<bool> = {
            let mut r = Xorshift::new(7);
            (0..32).map(|_| r.next_bool()).collect()
        };
        let b: Vec<bool> = {
            let mut r = Xorshift::new(7);
            (0..32).map(|_| r.next_bool()).collect()
        };
        assert_eq!(a, b, "same seed, same sequence");
        assert!(a.iter().any(|&x| x) && a.iter().any(|&x| !x));
    }
}
