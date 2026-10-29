// Offline follow-up diagnostics. Run with run-loudness-followup-probe.ps1.
// Assertions reproduce remaining defects; they are not desired-behavior tests.
use fletcher_core::config::{ConfigDoc, Parsed, FilterKind};
use fletcher_core::dsp::{self, FilterSpec};
#[derive(Default)]
struct OutsidePath { preamp_db: f64, filters: Vec<FilterSpec> }
const FS: f64 = 48000.0;
const RANGE: (f64, f64) = (-120.0, 12.0);

fn fixture(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("fletcher-review-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (file, text) in files {
        let path = dir.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir
}
fn read(dir: &std::path::Path) -> OutsidePath {
    let mut out = OutsidePath::default();
    walk_config(dir, "config.txt", (false, false), 0, &mut vec![], &mut out);
    out
}
fn pk(gain: f64) -> FilterSpec {
    FilterSpec { kind: FilterKind::Peaking, fc_hz: 1000.0, gain_db: gain, q: 1.0 }
}
#[test]
fn unrelated_scoped_cut_authorizes_unsafe_boost() {
    let dir = fixture("scope", &[("config.txt", "Device: OtherDevice\nFilter: ON PK Fc 1000 Hz Gain -12 dB Q 1\nDevice: all\nInclude: fletcher.txt\n")]);
    let out = read(&dir);
    let (p, _) = dsp::plan_level(&[pk(12.0)], &out.filters, FS, 0.0, out.preamp_db, true, RANGE);
    let actual = dsp::chain_response_db(&[pk(12.0)], p, FS, &[1000.0])[0];
    eprintln!("unrelated cut: preamp={p:.2}, actual center gain={actual:.4} dB");
    assert!(actual > 6.0);
    std::fs::remove_file(dir.join("config.txt")).unwrap();
    std::fs::remove_dir(dir).unwrap();
}
#[test]
fn repeated_include_is_not_a_cycle() {
    let dir = fixture("repeat", &[("config.txt", "Include: gain.txt\nInclude: gain.txt\nInclude: fletcher.txt\n"), ("gain.txt", "Preamp: 6 dB\n")]);
    let out = read(&dir);
    let (p, _) = dsp::plan_level(&[], &out.filters, FS, 0.0, out.preamp_db, true, RANGE);
    eprintln!("repeated include: modeled outside={}, actual outside=12, preamp={p}, residual={}", out.preamp_db, p+12.0);
    assert_eq!(out.preamp_db, 6.0);
    assert!(p + 12.0 > 5.9);
    for f in ["config.txt", "gain.txt"] { std::fs::remove_file(dir.join(f)).unwrap(); }
    std::fs::remove_dir(dir).unwrap();
}
#[test]
fn studio_exclusive_flat_ignores_pair_shortfall() {
    let a = [pk(12.0)];
    let pa = dsp::plan_level(&a, &[], FS, 0.0, 0.0, true, RANGE);
    let flat = dsp::plan_level(&[], &[], FS, 0.0, 0.0, true, RANGE);
    let (a_gain, paired_flat, shortfall, matched) = dsp::pair_levels(pa, flat, RANGE);
    assert!(matched);
    // exclusive_listening_db uses the solo Flat plan; apply_side uses the pair.
    let studio_gap = flat.0 - (dsp::loudness_db(&a, FS) + a_gain);
    eprintln!("Studio capped: solo Flat={}, paired Flat={paired_flat}, pair shortfall={shortfall}, bypass/EQ gap={studio_gap:.4} dB", flat.0);
    assert!(studio_gap > 5.0);
}

#[test]
fn nested_include_resolves_from_wrong_directory() {
    let dir = fixture("nested", &[("config.txt", "Include: sub/chain.txt\n"),
        ("sub/chain.txt", "Include: gain.txt\n"), ("sub/gain.txt", "Preamp: 12 dB\n")]);
    let out = read(&dir);
    eprintln!("nested include: modeled outside={}, actual outside=12", out.preamp_db);
    assert_eq!(out.preamp_db, 0.0);
    for f in ["config.txt", "sub/chain.txt", "sub/gain.txt"] { std::fs::remove_file(dir.join(f)).unwrap(); }
    std::fs::remove_dir(dir.join("sub")).unwrap();
    std::fs::remove_dir(dir).unwrap();
}

#[test]
fn infeasible_history_curve_does_not_describe_preview() {
    let a = vec![pk(30.0); 4];
    let pa = dsp::plan_level(&a, &[], FS, -50.0, 0.0, true, RANGE);
    let pb = dsp::plan_level(&[], &[], FS, -50.0, 0.0, true, RANGE);
    let (played_a, played_b, _, matched) = dsp::pair_levels(pa, pb, RANGE);
    let plotted_offset = pa.0 + pa.1 - pb.0 - pb.1;
    let actual_offset = played_a - played_b;
    eprintln!("infeasible preview: matched={matched}, plotted offset={plotted_offset}, played offset={actual_offset}, error={}", plotted_offset-actual_offset);
    assert!(!matched);
    assert!((plotted_offset-actual_offset).abs() > 10.0);
}
