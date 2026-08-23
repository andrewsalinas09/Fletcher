//! The lossless guarantee (ADR-0007 / Q-09): any file we parse — including
//! ones full of grammar we don't understand — serializes back byte-for-byte.
//! Fixtures are real files from the dev machine's live Equalizer APO install.

use fletcher_core::config::{ConfigDoc, FilterKind, Parsed};

const FIXTURES: &[(&str, &str)] = &[
    ("peace_hd650", include_str!("fixtures/peace_hd650.txt")),
    ("config_root", include_str!("fixtures/config_root.txt")),
    ("demo_rew", include_str!("fixtures/demo.txt")),
    ("example_rew", include_str!("fixtures/example.txt")),
    ("multichannel", include_str!("fixtures/multichannel.txt")),
    (
        "selective_delay",
        include_str!("fixtures/selective_delay.txt"),
    ),
];

#[test]
fn every_fixture_roundtrips_byte_for_byte() {
    for (name, text) in FIXTURES {
        let doc = ConfigDoc::parse(text);
        assert_eq!(&doc.to_string(), text, "round-trip failed for {name}");
    }
}

#[test]
fn tricky_endings_roundtrip() {
    for text in [
        "",
        "\n",
        "Preamp: -6 dB",     // no trailing newline
        "Preamp: -6 dB\r\n", // CRLF
        "a\nb\r\nc",         // mixed terminators
        "Filter 1: ON PK Fc 100 Hz Gain 3 dB Q 1\n\n\n",
    ] {
        assert_eq!(ConfigDoc::parse(text).to_string(), text);
    }
}

#[test]
fn hd650_preset_parses_semantically() {
    let doc = ConfigDoc::parse(include_str!("fixtures/peace_hd650.txt"));

    let preamps: Vec<f64> = doc
        .directives()
        .filter_map(|d| match d {
            Parsed::Preamp { db } => Some(*db),
            _ => None,
        })
        .collect();
    assert_eq!(preamps, vec![-8.1]);

    let filters: Vec<_> = doc
        .directives()
        .filter_map(|d| match d {
            Parsed::Filter {
                index,
                enabled,
                kind,
                fc_hz,
                gain_db,
                q,
            } => Some((*index, *enabled, *kind, *fc_hz, *gain_db, *q)),
            _ => None,
        })
        .collect();
    assert_eq!(filters.len(), 11, "Peace wrote 11 filters");
    assert!(filters.iter().all(|f| f.1), "all filters ON");

    // Filter 1: ON LSC Fc 105 Hz Gain 6.4 dB Q 0.7
    assert_eq!(
        filters[0],
        (
            Some(1),
            true,
            FilterKind::LowShelfC,
            Some(105.0),
            Some(6.4),
            Some(0.7)
        )
    );
    // Filter 6: ON HSC Fc 10000 Hz Gain -2.1 dB Q 0.7
    assert_eq!(filters[5].2, FilterKind::HighShelfC);
    assert_eq!(filters[5].4, Some(-2.1));
}

#[test]
fn root_config_sees_both_includes() {
    let doc = ConfigDoc::parse(include_str!("fixtures/config_root.txt"));
    let includes: Vec<&str> = doc
        .directives()
        .filter_map(|d| match d {
            Parsed::Include { path } => Some(path.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(includes, vec!["peace.txt", "fletcher.txt"]);
}

#[test]
fn rew_european_numbers_never_half_parse_as_wrong_values() {
    // demo.txt is a European-locale REW export: decimal commas ("50,0") and
    // dot thousands-separators ("8.000" = 8000 Hz). TB-22: comma-decimal
    // lines must degrade to Unknown, never misparse. (The "8.000" ambiguity
    // is only resolvable with REW-dialect detection — future work; losslessness
    // protects the file's content either way.)
    let doc = ConfigDoc::parse(include_str!("fixtures/demo.txt"));
    for line in &doc.lines {
        if line.raw.contains(',') && line.raw.trim_start().starts_with("Filter") {
            assert_eq!(
                line.parsed,
                Parsed::Unknown,
                "comma-decimal line must not half-parse: {:?}",
                line.raw
            );
        }
    }
}

#[test]
fn bare_filter_without_index_or_gain_parses() {
    // The exact line used in the hot-reload spike.
    let doc = ConfigDoc::parse("Filter: ON LP Fc 500 Hz\n");
    match doc.directives().next() {
        Some(Parsed::Filter {
            index: None,
            enabled: true,
            kind: FilterKind::LowPass,
            fc_hz: Some(fc),
            gain_db: None,
            q: None,
        }) => assert_eq!(*fc, 500.0),
        other => panic!("unexpected parse: {other:?}"),
    }
}

#[test]
fn malformed_lines_degrade_to_unknown_never_panic() {
    for bad in [
        "Filter 1: ON XYZ Fc 100 Hz",            // unknown filter type
        "Filter one: ON PK Fc 100 Hz",           // non-numeric index
        "Filter 1: MAYBE PK Fc 100 Hz",          // bad enable token
        "Preamp: loud",                          // non-numeric preamp
        "Filter 1: ON PK Fc 100 Hz Slope 12 dB", // unsupported param
        "just some words",
        "Filter 1:",
    ] {
        let text = format!("{bad}\n");
        let doc = ConfigDoc::parse(&text);
        assert_eq!(doc.to_string(), text, "lossless even when malformed: {bad}");
        assert!(
            doc.directives().next().is_none(),
            "should not half-parse: {bad}"
        );
    }
}
