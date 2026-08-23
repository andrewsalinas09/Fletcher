//! Test statistics for the Listening Lab.
//!
//! Discrimination (ABX) verdicts use the exact one-sided binomial test:
//! the probability of scoring at least `correct` out of `trials` by pure
//! guessing (p = 0.5). No approximations — trial counts are small.

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

/// A tiny deterministic RNG (xorshift64*) — no external dependency, good
/// enough for trial assignment. Seed from the caller (e.g. wall clock).
pub struct Xorshift(u64);

impl Xorshift {
    pub fn new(seed: u64) -> Self {
        Xorshift(seed.max(1))
    }

    pub fn next_bool(&mut self) -> bool {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x.wrapping_mul(0x2545F4914F6CDD1D) >> 63) == 1
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
