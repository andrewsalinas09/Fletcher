//! Room EQ Wizard filter import (GitHub issue #1): REW's "Export filter settings
//! as text" file, Generic equaliser.
//!
//! The rule is **reproduce what Equalizer APO would play** if the file were
//! Included, because that's what REW users have been doing until now. Every
//! number and default below comes from APO's own parser, not from guesses
//! (docs/research/apo-filter-semantics.md): commas are decimal points, an Fc
//! like `8.000` is 8000 Hz, `Modal` is a peaking filter, a Q-less notch has
//! Q 30, and the shelf variants (`LS`, `LS 12dB`, `HS 6dB`, …) each resolve to
//! a center frequency and Q.
//!
//! Output uses only kinds Fletcher draws exactly as APO plays them (PK, LPQ,
//! HPQ, BP, NO, AP, LSC, HSC, always with an explicit Q). So a REW `LS 12dB` at
//! 2000 Hz arrives as an `LSC` at ~2310 Hz: the same biquad, written the one
//! way both programs read identically (TB-34). Nothing is dropped silently:
//! whatever can't be reproduced exactly is returned in `skipped` with a reason.

use crate::config::{ChainFilter, FilterKind};
use crate::dsp::shelf_center_q;
use std::f64::consts::FRAC_1_SQRT_2;

#[derive(Debug, Clone, PartialEq)]
pub struct RewSkipped {
    /// 1-based line number in the file.
    pub line: usize,
    pub text: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct RewImport {
    pub filters: Vec<ChainFilter>,
    pub skipped: Vec<RewSkipped>,
    /// The `Equaliser:` header (e.g. "Generic"), if present.
    pub equaliser: Option<String>,
}

/// REW's text export starts with "Filter Settings file" and names its version
/// ("Room EQ V5.31.3"). Either marks the REW dialect.
pub fn looks_like_rew(text: &str) -> bool {
    text.lines().take(10).any(|l| {
        let l = l.trim();
        l.starts_with("Filter Settings file") || l.starts_with("Room EQ V")
    })
}

pub fn parse_rew(text: &str) -> RewImport {
    let mut out = RewImport::default();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if let Some(eq) = line.strip_prefix("Equaliser:") {
            out.equaliser = Some(eq.trim().to_string());
            continue;
        }
        let Some(params) = filter_params(line) else {
            continue;
        };
        match parse_filter(&params) {
            Ok(Some(f)) => out.filters.push(f),
            Ok(None) => {} // REW's empty "None" slots
            Err(reason) => out.skipped.push(RewSkipped {
                line: i + 1,
                text: line.to_string(),
                reason,
            }),
        }
    }
    out
}

/// `Filter  1: ON  PK …` → `ON  PK …` with APO's number normalisation applied.
/// Header lines like "Filter Settings file" have no `N:` and are not filters.
fn filter_params(line: &str) -> Option<String> {
    let rest = line.strip_prefix("Filter")?;
    let (index, params) = rest.split_once(':')?;
    if !index.trim().chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    // APO: every comma is a decimal point; non-breaking spaces are thousands
    // separators.
    Some(params.replace(',', ".").replace('\u{a0}', ""))
}

/// Leading numeric part of a token ("100H" → "100", REW's own demo file
/// writes `100H z`).
fn numeric_prefix(tok: &str) -> &str {
    let end = tok
        .find(|c: char| !(c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | 'e' | 'E')))
        .unwrap_or(tok.len());
    &tok[..end]
}

/// The number following `key`. The key must be followed by a number to count:
/// REW 5.31 writes two-word types like `LP Q` / `HS Q`, whose bare `Q` is part
/// of the name, not the parameter.
fn value_after<'a>(tokens: &[&'a str], key: &str) -> Option<&'a str> {
    tokens.windows(2).find_map(|w| {
        let n = numeric_prefix(w[1]);
        (w[0] == key && !n.is_empty()).then_some(n)
    })
}

/// APO's Fc rule: ≥ 5 chars, no exponent, and a '.' fourth from the end means
/// a thousands separator ("8.000" → 8000), because REW writes them that way.
fn parse_fc(s: &str) -> Option<f64> {
    let v: f64 = s.parse().ok()?;
    let b = s.as_bytes();
    let thousands = b.len() >= 5 && !s.contains(['e', 'E']) && b[b.len() - 4] == b'.';
    Some(if thousands { v * 1000.0 } else { v })
}

fn parse_filter(params: &str) -> Result<Option<ChainFilter>, String> {
    let tokens: Vec<&str> = params.split_whitespace().collect();
    let enabled = match tokens.first() {
        Some(&"ON") => true,
        Some(&"OFF") => false,
        _ => return Err("not an ON/OFF filter line".into()),
    };
    let mut type_code = *tokens.get(1).ok_or("no filter type")?;
    if type_code == "None" {
        return Ok(None);
    }
    // REW 5.31 names APO can't read (it takes letters up to the first space,
    // finds no such type, and plays nothing). REW's intent is unambiguous, so
    // honour it with the APO spelling. Verified against a real 5.31.3 export
    // (tests/fixtures/rew_5_31_generic.txt).
    match (type_code, tokens.get(2).copied()) {
        ("Notch", _) => type_code = "NO",
        ("All", Some("pass")) => type_code = "AP",
        // A first-order (6 dB/oct) filter: APO's Filter command has none, and
        // reads "LP1" as a second-order LP, a different filter.
        ("LP1" | "HP1", _) => {
            return Err(format!(
                "first-order {} — Equalizer APO's filters are all second-order, so it can't be reproduced",
                if type_code == "LP1" {
                    "low-pass"
                } else {
                    "high-pass"
                }
            ));
        }
        ("L-T", _) => return Err("Linkwitz transform isn't supported".into()),
        _ => {}
    }
    // A slope like "12dB" directly after the type: LS 12dB, HSC 6dB.
    let slope_db = tokens
        .get(2)
        .and_then(|t| t.strip_suffix("dB"))
        .and_then(|n| n.parse::<f64>().ok());

    let fc = value_after(&tokens, "Fc")
        .and_then(parse_fc)
        .filter(|f| f.is_finite() && *f > 0.0)
        .ok_or("no usable frequency")?;
    let gain = value_after(&tokens, "Gain").and_then(|g| g.parse::<f64>().ok());
    // APO treats Q 0 as "not given".
    let q = value_after(&tokens, "Q")
        .and_then(|q| q.parse::<f64>().ok())
        .filter(|q| *q != 0.0);
    if tokens.contains(&"BW") {
        // α depends on the sample rate for BW, so no fixed Q equals it.
        return Err("bandwidth (BW Oct) filters can't be reproduced exactly; export with Q".into());
    }

    let simple = |kind: FilterKind, gain_db: f64, q: f64| {
        Ok(Some(ChainFilter {
            enabled,
            kind,
            fc_hz: fc,
            gain_db,
            q,
        }))
    };
    match type_code {
        "PK" | "PEQ" | "Modal" => {
            let g = gain.ok_or("peaking filter without a gain")?;
            let q = q.ok_or("peaking filter without a Q")?;
            simple(FilterKind::Peaking, g, q)
        }
        "LP" | "LPQ" => simple(FilterKind::LowPassQ, 0.0, q.unwrap_or(FRAC_1_SQRT_2)),
        "HP" | "HPQ" => simple(FilterKind::HighPassQ, 0.0, q.unwrap_or(FRAC_1_SQRT_2)),
        "BP" => simple(FilterKind::BandPass, 0.0, q.unwrap_or(FRAC_1_SQRT_2)),
        "NO" => simple(FilterKind::Notch, 0.0, q.unwrap_or(30.0)),
        "AP" => simple(
            FilterKind::AllPass,
            0.0,
            q.ok_or("all-pass filter without a Q")?,
        ),
        "LS" | "LSC" | "HS" | "HSC" => {
            let g = gain.ok_or("shelf filter without a gain")?;
            let low = type_code.starts_with('L');
            let (fc, q) = shelf_center_q(low, type_code.ends_with('C'), fc, g, q, slope_db)
                .ok_or("shelf parameters outside what the filter can represent")?;
            Ok(Some(ChainFilter {
                enabled,
                kind: if low {
                    FilterKind::LowShelfC
                } else {
                    FilterKind::HighShelfC
                },
                fc_hz: fc,
                gain_db: g,
                q,
            }))
        }
        other => Err(format!(
            "\"{other}\" isn't a filter type Equalizer APO plays (Generic equaliser types only)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::Biquad;

    const FS: f64 = 48000.0;

    /// Equalizer APO's shelf, ported line for line from BiQuad.cpp +
    /// BiQuadFilter.cpp: the reference the conversion must match.
    fn apo_shelf_db(
        low: bool,
        gain: f64,
        freq: f64,
        q_or_s: f64,
        is_s: bool,
        corner: bool,
        f: f64,
    ) -> f64 {
        let a = 10f64.powf(gain / 40.0);
        let mut freq = freq;
        if corner {
            let s = if is_s {
                q_or_s
            } else {
                1.0 / ((1.0 / (q_or_s * q_or_s) - 2.0) / (a + 1.0 / a) + 1.0)
            };
            let k = 10f64.powf(gain.abs() / 80.0 / s);
            freq = if low { freq * k } else { freq / k };
        }
        let w = 2.0 * std::f64::consts::PI * freq / FS;
        let (sn, cs) = w.sin_cos();
        let alpha = if is_s {
            sn / 2.0 * ((a + 1.0 / a) * (1.0 / q_or_s - 1.0) + 2.0).sqrt()
        } else {
            sn / (2.0 * q_or_s)
        };
        let beta = 2.0 * a.sqrt() * alpha;
        let (b0, b1, b2, a0, a1, a2) = if low {
            (
                a * ((a + 1.0) - (a - 1.0) * cs + beta),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cs),
                a * ((a + 1.0) - (a - 1.0) * cs - beta),
                (a + 1.0) + (a - 1.0) * cs + beta,
                -2.0 * ((a - 1.0) + (a + 1.0) * cs),
                (a + 1.0) + (a - 1.0) * cs - beta,
            )
        } else {
            (
                a * ((a + 1.0) + (a - 1.0) * cs + beta),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cs),
                a * ((a + 1.0) + (a - 1.0) * cs - beta),
                (a + 1.0) - (a - 1.0) * cs + beta,
                2.0 * ((a - 1.0) - (a + 1.0) * cs),
                (a + 1.0) - (a - 1.0) * cs - beta,
            )
        };
        Biquad {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
        .magnitude_db(f, FS)
    }

    fn fletcher_db(c: &ChainFilter, f: f64) -> f64 {
        Biquad::rbj(c.kind, FS, c.fc_hz, c.gain_db, c.q).magnitude_db(f, FS)
    }

    fn grid() -> impl Iterator<Item = f64> {
        (0..200).map(|i| 20.0 * 1000f64.powf(i as f64 / 199.0))
    }

    #[test]
    fn every_shelf_spelling_matches_apo_exactly() {
        // (line, low, gain, freq, q_or_s, is_s, corner) — what APO builds for it.
        let cases = [
            (
                "Filter 1: ON LS Fc 300 Hz Gain 5.0 dB",
                true,
                5.0,
                300.0,
                0.9,
                true,
                false,
            ),
            (
                "Filter 1: ON HS Fc 1000 Hz Gain -3.0 dB",
                false,
                -3.0,
                1000.0,
                0.9,
                true,
                false,
            ),
            (
                "Filter 1: ON LS 12dB Fc 2000 Hz Gain -5.0 dB",
                true,
                -5.0,
                2000.0,
                1.0,
                true,
                true,
            ),
            (
                "Filter 1: ON HS 12dB Fc 500 Hz Gain 5.0 dB",
                false,
                5.0,
                500.0,
                1.0,
                true,
                true,
            ),
            (
                "Filter 1: ON LS 6dB Fc 50.0 Hz Gain 7.2 dB",
                true,
                7.2,
                50.0,
                0.5,
                true,
                true,
            ),
            (
                "Filter 1: ON HS 6dB Fc 12000 Hz Gain 10.0 dB",
                false,
                10.0,
                12000.0,
                0.5,
                true,
                true,
            ),
            (
                "Filter 1: ON LS Fc 100 Hz Gain 6 dB Q 0.9",
                true,
                6.0,
                100.0,
                0.9,
                false,
                true,
            ),
            (
                "Filter 1: ON HSC Fc 4000 Hz Gain -4 dB Q 0.6",
                false,
                -4.0,
                4000.0,
                0.6,
                false,
                false,
            ),
            (
                "Filter 1: ON LSC 6dB Fc 120 Hz Gain 3 dB",
                true,
                3.0,
                120.0,
                0.5,
                true,
                false,
            ),
            // REW 5.31's two-word spelling: APO reads "LS", then the Q → corner shelf.
            (
                "Filter 15: ON  LS Q     Fc   500.0 Hz  Gain   6.00 dB Q  0.707",
                true,
                6.0,
                500.0,
                0.707,
                false,
                true,
            ),
            (
                "Filter 16: ON  HS Q     Fc   630.0 Hz  Gain  -9.00 dB Q  1.100",
                false,
                -9.0,
                630.0,
                1.1,
                false,
                true,
            ),
        ];
        for (line, low, gain, freq, qs, is_s, corner) in cases {
            let got = parse_rew(line);
            assert!(got.skipped.is_empty(), "{line}: {:?}", got.skipped);
            let c = &got.filters[0];
            for f in grid() {
                let want = apo_shelf_db(low, gain, freq, qs, is_s, corner, f);
                let have = fletcher_db(c, f);
                assert!(
                    (want - have).abs() < 1e-9,
                    "{line} @ {f:.1} Hz: APO {want} vs Fletcher {have}"
                );
            }
        }
    }

    #[test]
    fn rews_european_demo_file_reads_as_apo_plays_it() {
        // The REW sample export shipped with APO: decimal commas, dot thousands,
        // every Generic type, six empty slots, and REW's own "100H z" typo.
        let got = parse_rew(include_str!("../tests/fixtures/demo.txt"));
        assert_eq!(got.equaliser.as_deref(), Some("Generic"));
        assert!(got.skipped.is_empty(), "{:?}", got.skipped);
        let summary: Vec<(&str, f64)> = got
            .filters
            .iter()
            .map(|f| (f.kind.code(), f.fc_hz))
            .collect();
        let kinds: Vec<&str> = summary.iter().map(|s| s.0).collect();
        assert_eq!(
            kinds,
            [
                "PK", "PK", "LPQ", "HPQ", "LPQ", "HPQ", "LSC", "HSC", "LSC", "HSC", "LSC", "HSC",
                "NO", "AP"
            ]
        );
        let f = &got.filters;
        assert_eq!((f[0].fc_hz, f[0].gain_db, f[0].q), (50.0, -10.0, 2.5));
        assert_eq!((f[1].fc_hz, f[1].gain_db, f[1].q), (100.0, 3.0, 5.41)); // Modal → PK, T60 ignored
        assert_eq!(f[2].fc_hz, 8000.0); // "8.000"
        assert_eq!((f[4].fc_hz, f[4].q), (10000.0, 0.4)); // "10.000", "0,400"
        assert_eq!(f[12].q, 30.0); // Q-less notch
        assert!((f[8].fc_hz - 2000.0 * 10f64.powf(5.0 / 80.0)).abs() < 1e-9); // LS 12dB corner → center
    }

    #[test]
    fn rew_5_31_real_export_every_generic_type() {
        // Exported by the user from REW 5.31.3 (Generic equaliser, one of every
        // type). Newer than demo.txt: two-word names ("LP Q", "Notch Q",
        // "All pass") and first-order / Linkwitz types APO can't play.
        let got = parse_rew(include_str!("../tests/fixtures/rew_5_31_generic.txt"));
        let kinds: Vec<&str> = got.filters.iter().map(|f| f.kind.code()).collect();
        assert_eq!(
            kinds,
            [
                "PK", "PK", "LPQ", "HPQ", "LPQ", "HPQ", "LSC", "HSC", "LSC", "HSC", "LSC", "HSC",
                "LSC", "HSC", "NO", "NO", "AP"
            ]
        );
        let f = &got.filters;
        assert_eq!((f[1].fc_hz, f[1].q), (25.0, 3.411)); // Modal
        assert_eq!((f[4].fc_hz, f[4].q), (100.0, 0.707)); // "LP Q" — the bare Q is part of the name
        assert_eq!((f[14].fc_hz, f[14].q), (800.0, 30.0)); // "Notch", no Q
        assert_eq!((f[15].fc_hz, f[15].q), (100.0, 0.707)); // "Notch Q"
        assert_eq!((f[16].fc_hz, f[16].q), (100.0, 0.707)); // "All pass"
        let skipped: Vec<(usize, bool)> = got
            .skipped
            .iter()
            .map(|s| {
                (
                    s.line,
                    s.reason.contains("first-order") || s.reason.contains("Linkwitz"),
                )
            })
            .collect();
        assert_eq!(skipped, [(14, true), (15, true), (29, true)]); // LP1, HP1, L-T
    }

    #[test]
    fn unplayable_lines_are_reported_never_dropped() {
        let got = parse_rew(
            "Filter Settings file\n\
             Filter  1: ON  PK  Fc 100 Hz Gain -3 dB Q 2\n\
             Filter  2: ON  XO  Fc 80 Hz\n\
             Filter  3: ON  PK  Fc 200 Hz Gain -3 dB BW Oct 0.5\n\
             Filter  4: ON  PK  Fc 300 Hz Gain -3 dB\n\
             Filter  5: OFF PK  Fc 400 Hz Gain 2 dB Q 1\n\
             Filter  6: ON  None\n",
        );
        assert_eq!(got.filters.len(), 2);
        assert!(!got.filters[1].enabled, "OFF filters import disabled");
        let lines: Vec<usize> = got.skipped.iter().map(|s| s.line).collect();
        assert_eq!(lines, [3, 4, 5]);
    }

    #[test]
    fn detects_rew_dialect_by_header_only() {
        assert!(looks_like_rew("Filter Settings file\n\nRoom EQ V5.31.3\n"));
        assert!(!looks_like_rew(
            "Filter 1: ON PK Fc 100 Hz Gain -3 dB Q 2\n"
        ));
    }
}
