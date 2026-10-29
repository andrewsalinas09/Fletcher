//! ADR-0016 validation: `dsp::loudness_db` (the system path's level estimate)
//! must agree with real LUFS, the measurement the clip-trial path uses, so the
//! two ways Fletcher matches levels mean the same thing. Pink noise is pushed
//! through each chain and measured with ebur128; the estimate must land within
//! a tenth of a dB or so. The old estimator (mean of the dB curve) is printed
//! alongside for the record: it missed treble boosts by about 1 dB.

use fletcher_core::config::FilterKind::{self, *};
use fletcher_core::dsp::{ChainProcessor, FilterSpec, chain_response_db, log_freqs, loudness_db};
use fletcher_core::signal::{Signal, SignalKind};

const FS: f64 = 48000.0;
const SECONDS: usize = 30;

fn spec(kind: FilterKind, fc_hz: f64, gain_db: f64, q: f64) -> FilterSpec {
    FilterSpec {
        kind,
        fc_hz,
        gain_db,
        q,
    }
}

/// Integrated LUFS of the same pink noise (fixed seed) through `filters`.
fn lufs_of(filters: &[FilterSpec]) -> f64 {
    let mut noise = Signal::new(SignalKind::Pink, 0.25, FS, 7);
    let mut chain = ChainProcessor::new(filters, 0.0, FS);
    let mut meter = ebur128::EbuR128::new(2, FS as u32, ebur128::Mode::I).unwrap();
    let mut block = Vec::with_capacity(2 * 4800);
    for _ in 0..SECONDS * 10 {
        block.clear();
        for _ in 0..4800 {
            let x = noise.next_sample();
            let (l, r) = chain.process_frame(x, x);
            block.push(l as f32);
            block.push(r as f32);
        }
        meter.add_frames_f32(&block).unwrap();
    }
    meter.loudness_global().unwrap()
}

fn mean_db(filters: &[FilterSpec]) -> f64 {
    let freqs = log_freqs(20.0, 20000.0, 200);
    let r = chain_response_db(filters, 0.0, FS, &freqs);
    r.iter().sum::<f64>() / r.len() as f64
}

#[test]
fn loudness_estimate_matches_measured_lufs() {
    let flat = lufs_of(&[]);
    let cases: Vec<(&str, Vec<FilterSpec>)> = vec![
        ("+6 PK 3k Q1", vec![spec(Peaking, 3000.0, 6.0, 1.0)]),
        ("+10 PK 3k Q4", vec![spec(Peaking, 3000.0, 10.0, 4.0)]),
        ("+6 HSC 8k", vec![spec(HighShelfC, 8000.0, 6.0, 0.7)]),
        ("+8 LSC 105", vec![spec(LowShelfC, 105.0, 8.0, 0.7)]),
        ("-6 PK 1k Q2", vec![spec(Peaking, 1000.0, -6.0, 2.0)]),
        (
            "V-shape",
            vec![
                spec(LowShelfC, 100.0, 6.0, 0.7),
                spec(Peaking, 1500.0, -3.0, 0.8),
                spec(HighShelfC, 9000.0, 5.0, 0.7),
            ],
        ),
    ];
    for (name, chain) in &cases {
        let measured = lufs_of(chain) - flat;
        let estimate = loudness_db(chain, FS);
        let old = mean_db(chain);
        eprintln!(
            "{name:>14}: measured {measured:+.2}  estimate {estimate:+.2}  old mean-dB {old:+.2}"
        );
        assert!(
            (estimate - measured).abs() < 0.1,
            "{name}: estimate {estimate:.2} vs measured {measured:.2} dB"
        );
    }
}
