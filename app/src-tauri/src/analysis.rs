//! Spectral analysis for the Clip Studio scopes (M5): the whole-track
//! spectrogram (log-frequency tile grid, u8 dB magnitudes) and the
//! instantaneous spectrum at the playhead for the FFT pane. Rust computes,
//! the UI draws (ADR-0005).

use std::sync::Arc;

use realfft::RealFftPlanner;

/// dB range mapped onto u8 0..255 in the spectrogram tiles.
pub const SPEC_DB_MIN: f64 = -90.0;
pub const SPEC_DB_MAX: f64 = -10.0;

pub struct Spectrogram {
    pub cols: usize,
    pub rows: usize,
    /// Row-major, row 0 = LOWEST frequency; u8 = dB position in
    /// [SPEC_DB_MIN, SPEC_DB_MAX].
    pub data: Vec<u8>,
}

/// Log-spaced band edges from `lo` to `hi` Hz, `rows + 1` edges.
fn band_edges(lo: f64, hi: f64, rows: usize) -> Vec<f64> {
    let (llo, lhi) = (lo.log10(), hi.log10());
    (0..=rows)
        .map(|i| 10f64.powf(llo + (lhi - llo) * i as f64 / rows as f64))
        .collect()
}

fn hann(n: usize) -> Vec<f64> {
    (0..n)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos())
        .collect()
}

/// Whole-track spectrogram: `cols` FFT snapshots (window `win`, a power of
/// two — bigger = finer frequency, blurrier time) mapped into `rows` log
/// bands 20 Hz – 20 kHz; `floor_db` sets the u8 contrast floor.
pub fn spectrogram(
    pcm: &Arc<Vec<f32>>,
    rate: u32,
    cols: usize,
    rows: usize,
    win: usize,
    floor_db: f64,
) -> Spectrogram {
    let win = win.clamp(256, 16384).next_power_of_two();
    let floor_db = floor_db.clamp(-140.0, SPEC_DB_MAX - 10.0);
    let frames = pcm.len() / 2;
    let cols = cols.clamp(64, 8000).min(frames.max(1));
    let mut planner = RealFftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(win);
    let window = hann(win);
    let edges = band_edges(20.0, 20000.0f64.min(rate as f64 / 2.0), rows);
    let bin_hz = rate as f64 / win as f64;

    let mut data = vec![0u8; cols * rows];
    let mut input = fft.make_input_vec();
    let mut output = fft.make_output_vec();
    for c in 0..cols {
        let center = (c * frames) / cols;
        let start = center
            .saturating_sub(win / 2)
            .min(frames.saturating_sub(win.min(frames)));
        for i in 0..win {
            let f = start + i;
            let mono = if f < frames {
                (pcm[f * 2] as f64 + pcm[f * 2 + 1] as f64) * 0.5
            } else {
                0.0
            };
            input[i] = mono * window[i];
        }
        if fft.process(&mut input, &mut output).is_err() {
            continue;
        }
        for r in 0..rows {
            let b0 = ((edges[r] / bin_hz) as usize).max(1);
            let b1 = ((edges[r + 1] / bin_hz).ceil() as usize)
                .max(b0 + 1)
                .min(output.len());
            let mut peak = 0.0f64;
            for bin in output.iter().take(b1).skip(b0) {
                let m = bin.norm_sqr();
                if m > peak {
                    peak = m;
                }
            }
            // Amplitude normalized by window energy; dB → u8.
            let amp = peak.sqrt() * 2.0 / (win as f64 / 2.0);
            let db = 20.0 * amp.max(1e-12).log10();
            let t = ((db - floor_db) / (SPEC_DB_MAX - floor_db)).clamp(0.0, 1.0);
            data[r * cols + c] = (t * 255.0) as u8;
        }
    }
    Spectrogram { cols, rows, data }
}

/// Instantaneous spectrum at time `t_s`: `points` log-spaced dB values
/// 20 Hz – 20 kHz from a 4096-sample window (the FFT pane).
pub fn spectrum_at(pcm: &Arc<Vec<f32>>, rate: u32, t_s: f64, points: usize) -> Vec<f64> {
    const WIN: usize = 4096;
    let frames = pcm.len() / 2;
    let center = ((t_s * rate as f64) as usize).min(frames);
    let start = center
        .saturating_sub(WIN / 2)
        .min(frames.saturating_sub(WIN.min(frames)));
    let mut planner = RealFftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(WIN);
    let window = hann(WIN);
    let mut input = fft.make_input_vec();
    let mut output = fft.make_output_vec();
    for i in 0..WIN {
        let f = start + i;
        let mono = if f < frames {
            (pcm[f * 2] as f64 + pcm[f * 2 + 1] as f64) * 0.5
        } else {
            0.0
        };
        input[i] = mono * window[i];
    }
    if fft.process(&mut input, &mut output).is_err() {
        return vec![SPEC_DB_MIN; points];
    }
    let bin_hz = rate as f64 / WIN as f64;
    let edges = band_edges(20.0, 20000.0f64.min(rate as f64 / 2.0), points);
    (0..points)
        .map(|p| {
            let b0 = ((edges[p] / bin_hz) as usize).max(1);
            let b1 = ((edges[p + 1] / bin_hz).ceil() as usize)
                .max(b0 + 1)
                .min(output.len());
            let mut peak = 0.0f64;
            for bin in output.iter().take(b1).skip(b0) {
                let m = bin.norm_sqr();
                if m > peak {
                    peak = m;
                }
            }
            let amp = peak.sqrt() * 2.0 / (WIN as f64 / 2.0);
            (20.0 * amp.max(1e-12).log10()).max(SPEC_DB_MIN)
        })
        .collect()
}
