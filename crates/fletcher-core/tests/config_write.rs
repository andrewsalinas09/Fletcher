//! Writer-side guarantees: canonical rendering re-parses to the same values,
//! ensure_include is idempotent and style-preserving, atomic writes land whole.

use fletcher_core::config::{ConfigDoc, FilterKind, Parsed, format_filter, format_preamp};
use fletcher_core::fsx;

#[test]
fn formatted_filters_reparse_to_identical_values() {
    let cases = [
        // The HD650 preset's shape (LSC/PK/HSC with fractional everything)
        (
            Some(1),
            true,
            FilterKind::LowShelfC,
            Some(105.0),
            Some(6.4),
            Some(0.7),
        ),
        (
            Some(2),
            true,
            FilterKind::Peaking,
            Some(8800.0),
            Some(5.1),
            Some(1.42),
        ),
        (
            Some(6),
            true,
            FilterKind::HighShelfC,
            Some(10000.0),
            Some(-2.1),
            Some(0.7),
        ),
        // Spike line: no index, no gain, no Q
        (None, true, FilterKind::LowPass, Some(500.0), None, None),
        (
            Some(3),
            false,
            FilterKind::Notch,
            Some(60.0),
            None,
            Some(30.0),
        ),
    ];
    for (index, enabled, kind, fc, gain, q) in cases {
        let line = format_filter(index, enabled, kind, fc, gain, q);
        let doc = ConfigDoc::parse(&line);
        match doc.directives().next() {
            Some(Parsed::Filter {
                index: i,
                enabled: e,
                kind: k,
                fc_hz,
                gain_db,
                q: qq,
            }) => {
                assert_eq!(
                    (*i, *e, *k, *fc_hz, *gain_db, *qq),
                    (index, enabled, kind, fc, gain, q),
                    "line: {line}"
                );
            }
            other => panic!("{line:?} parsed as {other:?}"),
        }
    }
}

#[test]
fn formatted_preamp_reparses() {
    let doc = ConfigDoc::parse(&format_preamp(-8.1));
    assert!(matches!(doc.directives().next(), Some(Parsed::Preamp { db }) if *db == -8.1));
}

#[test]
fn ensure_include_is_idempotent_on_live_root_config() {
    // The real config.txt fixture already includes fletcher.txt.
    let text = include_str!("fixtures/config_root.txt");
    let mut doc = ConfigDoc::parse(text);
    assert!(!doc.ensure_include("fletcher.txt"), "must not double-add");
    assert!(!doc.ensure_include("FLETCHER.TXT"), "case-insensitive");
    assert_eq!(doc.to_string(), text, "no-op must not change a byte");
}

#[test]
fn ensure_include_appends_matching_line_style() {
    // CRLF file → CRLF appended.
    let mut doc = ConfigDoc::parse("Include: peace.txt\r\n");
    assert!(doc.ensure_include("fletcher.txt"));
    assert_eq!(
        doc.to_string(),
        "Include: peace.txt\r\nInclude: fletcher.txt\r\n"
    );

    // LF file → LF appended.
    let mut doc = ConfigDoc::parse("Include: peace.txt\n");
    assert!(doc.ensure_include("fletcher.txt"));
    assert_eq!(
        doc.to_string(),
        "Include: peace.txt\nInclude: fletcher.txt\n"
    );

    // Unterminated last line gets a terminator before the append.
    let mut doc = ConfigDoc::parse("Include: peace.txt");
    assert!(doc.ensure_include("fletcher.txt"));
    assert_eq!(
        doc.to_string(),
        "Include: peace.txt\r\nInclude: fletcher.txt\r\n"
    );

    // Empty file just gets the line.
    let mut doc = ConfigDoc::parse("");
    assert!(doc.ensure_include("fletcher.txt"));
    assert_eq!(doc.to_string(), "Include: fletcher.txt\r\n");
}

#[test]
fn push_line_parses_what_it_appends() {
    let mut doc = ConfigDoc::parse("");
    doc.push_line(&format_preamp(-3.0));
    doc.push_line(&format_filter(
        Some(1),
        true,
        FilterKind::Peaking,
        Some(1000.0),
        Some(-2.0),
        Some(1.0),
    ));
    assert_eq!(doc.directives().count(), 2);
    // And the result still round-trips through a fresh parse.
    let reparsed = ConfigDoc::parse(&doc.to_string());
    assert_eq!(reparsed.to_string(), doc.to_string());
}

#[test]
fn rendered_fletcher_file_parses_back_to_the_same_chain() {
    use fletcher_core::config::{ChainFilter, render_fletcher_file};
    let chain = [
        ChainFilter {
            enabled: true,
            kind: FilterKind::Peaking,
            fc_hz: 1000.0,
            gain_db: 3.5,
            q: 1.41,
        },
        ChainFilter {
            enabled: false,
            kind: FilterKind::LowShelfC,
            fc_hz: 105.0,
            gain_db: -2.0,
            q: 0.7,
        },
    ];
    let text = render_fletcher_file(-3.5, &chain);
    let doc = ConfigDoc::parse(&text);
    let filters: Vec<_> = doc
        .directives()
        .filter_map(|d| match d {
            Parsed::Filter {
                enabled,
                kind,
                fc_hz,
                gain_db,
                q,
                ..
            } => Some((
                *enabled,
                *kind,
                fc_hz.unwrap(),
                gain_db.unwrap(),
                q.unwrap(),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(filters.len(), 2);
    assert_eq!(filters[0], (true, FilterKind::Peaking, 1000.0, 3.5, 1.41));
    assert_eq!(filters[1], (false, FilterKind::LowShelfC, 105.0, -2.0, 0.7));
    assert!(
        doc.directives()
            .any(|d| matches!(d, Parsed::Preamp { db } if *db == -3.5))
    );
}

#[test]
fn auto_preamp_counts_filter_interaction() {
    use fletcher_core::dsp::{FilterSpec, auto_preamp_db};
    // Two overlapping +3 dB bells sum past 3 dB at the crossover region:
    // the naive max-single-gain rule (-3.0) is insufficient (TB-06).
    let overlapping = [
        FilterSpec {
            kind: FilterKind::Peaking,
            fc_hz: 900.0,
            gain_db: 3.0,
            q: 1.0,
        },
        FilterSpec {
            kind: FilterKind::Peaking,
            fc_hz: 1100.0,
            gain_db: 3.0,
            q: 1.0,
        },
    ];
    let p = auto_preamp_db(&overlapping, 48000.0);
    assert!(p < -3.0, "interaction must deepen preamp, got {p}");
    assert!(p > -6.5, "sanity bound, got {p}");
    // Pure cuts need no preamp.
    let cut = [FilterSpec {
        kind: FilterKind::Peaking,
        fc_hz: 1000.0,
        gain_db: -4.0,
        q: 1.0,
    }];
    assert_eq!(auto_preamp_db(&cut, 48000.0), 0.0);
}

#[test]
fn write_atomic_lands_whole_and_replaces() {
    let dir = std::env::temp_dir().join(format!("fletcher-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.txt");

    fsx::write_atomic(&path, "first\r\n").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "first\r\n");

    fsx::write_atomic(&path, "second\r\n").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "second\r\n");

    assert!(
        !dir.join("config.fletcher-tmp").exists(),
        "temp file must not linger"
    );
    std::fs::remove_dir_all(&dir).ok();
}
