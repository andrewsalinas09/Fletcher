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
