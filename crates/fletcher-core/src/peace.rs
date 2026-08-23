//! Importer for Peace's own .peace preset format (INI-style).
//!
//! Peace empties peace.txt when its EQ is toggled off, but keeps the live
//! chain in "Last Configuration.peace" — parsing that lets "copy from Peace"
//! work regardless of Peace's on/off state (Q-03 residual).
//!
//! Observed format (fixture: tests/fixtures/hd650.peace): flat keys
//! `FrequencyN` / `GainN` / `QualityN` in `[Frequencies]` / `[Gains]` /
//! `[Qualities]`; sparse `[Filters]` lists only non-default types
//! (14 = low shelf, 15 = high shelf, absent = peaking); `PreAmp` in
//! `[General]`. Numbered section variants ([Frequencies1]…) are Peace's
//! multi-speaker slots — ignored.

use crate::config::{ChainFilter, FilterKind};
use std::collections::HashMap;

pub struct PeaceImport {
    pub preamp_db: f64,
    pub filters: Vec<ChainFilter>,
    /// Filters whose Peace type code we don't know yet; imported as peaking.
    pub unknown_types: usize,
}

fn kind_from_peace_code(code: i64) -> Option<FilterKind> {
    match code {
        14 => Some(FilterKind::LowShelfC),
        15 => Some(FilterKind::HighShelfC),
        _ => None,
    }
}

pub fn parse_peace(text: &str) -> Option<PeaceImport> {
    let mut section = String::new();
    let mut freqs: HashMap<u32, f64> = HashMap::new();
    let mut gains: HashMap<u32, f64> = HashMap::new();
    let mut qs: HashMap<u32, f64> = HashMap::new();
    let mut types: HashMap<u32, i64> = HashMap::new();
    let mut preamp_db = 0.0;

    let numbered =
        |key: &str, prefix: &str| -> Option<u32> { key.strip_prefix(prefix)?.parse().ok() };

    for line in text.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            section = name.to_string();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match section.as_str() {
            "Frequencies" => {
                if let (Some(i), Ok(v)) = (numbered(key, "Frequency"), value.parse()) {
                    freqs.insert(i, v);
                }
            }
            "Gains" => {
                if let (Some(i), Ok(v)) = (numbered(key, "Gain"), value.parse()) {
                    gains.insert(i, v);
                }
            }
            "Qualities" => {
                if let (Some(i), Ok(v)) = (numbered(key, "Quality"), value.parse()) {
                    qs.insert(i, v);
                }
            }
            "Filters" => {
                if let (Some(i), Ok(v)) = (numbered(key, "Filter"), value.parse()) {
                    types.insert(i, v);
                }
            }
            "General" if key == "PreAmp" => {
                preamp_db = value.parse().unwrap_or(0.0);
            }
            _ => {}
        }
    }

    if freqs.is_empty() {
        return None;
    }
    let mut indices: Vec<u32> = freqs.keys().copied().collect();
    indices.sort_unstable();

    let mut unknown_types = 0;
    let filters = indices
        .into_iter()
        .map(|i| {
            let kind = match types.get(&i) {
                None => FilterKind::Peaking,
                Some(code) => kind_from_peace_code(*code).unwrap_or_else(|| {
                    unknown_types += 1;
                    FilterKind::Peaking
                }),
            };
            ChainFilter {
                enabled: true,
                kind,
                fc_hz: freqs[&i],
                gain_db: gains.get(&i).copied().unwrap_or(0.0),
                q: qs
                    .get(&i)
                    .copied()
                    .unwrap_or(std::f64::consts::FRAC_1_SQRT_2),
            }
        })
        .collect();

    Some(PeaceImport {
        preamp_db,
        filters,
        unknown_types,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_hd650_peace_preset() {
        let import = parse_peace(include_str!("../tests/fixtures/hd650.peace")).unwrap();
        assert_eq!(import.preamp_db, -8.1);
        assert_eq!(import.filters.len(), 11);
        assert_eq!(import.unknown_types, 0);

        // Filter1 = LSC 105 Hz +6.4 Q 0.7 (Peace type 14)
        let f1 = import.filters[0];
        assert_eq!(
            (f1.kind, f1.fc_hz, f1.gain_db, f1.q),
            (FilterKind::LowShelfC, 105.0, 6.4, 0.7)
        );
        // Filter6 = HSC 10 kHz −2.1 (Peace type 15)
        let f6 = import.filters[5];
        assert_eq!(
            (f6.kind, f6.fc_hz, f6.gain_db),
            (FilterKind::HighShelfC, 10000.0, -2.1)
        );
        // Filter11 = LSC 60 Hz +2 (Peace type 14)
        let f11 = import.filters[10];
        assert_eq!(
            (f11.kind, f11.fc_hz, f11.gain_db),
            (FilterKind::LowShelfC, 60.0, 2.0)
        );
        // Unlisted types default to peaking.
        assert_eq!(import.filters[1].kind, FilterKind::Peaking);
    }

    #[test]
    fn empty_or_garbage_yields_none() {
        assert!(parse_peace("").is_none());
        assert!(parse_peace("[General]\nPreAmp=-3\n").is_none());
        assert!(parse_peace("not an ini at all").is_none());
    }
}
