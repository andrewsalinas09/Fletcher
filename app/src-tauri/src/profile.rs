//! The mic-free hearing profile (Q-19 / ADR-0013): loudness-match octave
//! noise bands against a fixed 500 Hz anchor via timed 2AFC pairs, adaptive
//! PSE staircases per band, everything journaled (a session is exactly
//! reproducible from its seed + journal).
//!
//! Audio: ONE exclusive stream per block — the engine's mandatory 300 ms
//! ramp (TB-20) runs once against silence; intervals are gated in-signal so
//! interval 1 and interval 2 are shaped identically (a per-stream ramp would
//! be a TB-12-class asymmetry). All levels are desired-RMS dBFS on a
//! normalized axis (band RMS scales with √bandwidth — see signal::rms_dbfs).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use fletcher_core::playback::{self, OutputMode, Source};
use fletcher_core::signal::{self, Signal, SignalKind};
use fletcher_core::stats::{combine_pse, Staircase, StaircaseConfig, StaircaseEnd, Xorshift};

pub const INTERVAL_S: f64 = 0.8;
pub const GAP_S: f64 = 0.3;
pub const GATE_S: f64 = 0.05;
/// Excursion bound around the anchor (protects ears and headphones).
const EXCURSION_DB: f64 = 20.0;
/// Level-set starts here (TB-20: quiet, user raises).
const LEVEL_START_DBFS: f64 = -65.0;
/// Absolute RMS ceiling for any presented interval: peaks of LR4 band noise
/// sit ~13 dB above RMS, so -18 dBFS RMS keeps true peaks near -5 dBFS.
/// (The generator's MAX_AMP cap protects raw library items; the profile's
/// levels are staircase-bounded and user-anchored, so the honest ceiling is
/// peak headroom, not the library cap.)
pub const MAX_RMS_DBFS: f64 = -18.0;
/// Accepting an anchor louder than this leaves low bands little headroom
/// (surfaced as `levelWarn` in the state DTO).
pub const LEVEL_WARN_DBFS: f64 = -38.0;
/// One catch trial per this many presentations (slot jittered).
const CATCH_EVERY: u64 = 8;
const LAPSE_DELTA_DB: f64 = 10.0;

/// Band centers (Hz); 500 is the anchor and excluded from comparisons.
pub const ANCHOR_HZ: f64 = 500.0;
const BAND_CENTERS: [f64; 9] = [
    63.0, 125.0, 250.0, 1000.0, 2000.0, 4000.0, 8000.0, 12500.0, 16000.0,
];
/// Approximate ISO 226-shaped start seeds (Δ dB vs 500 Hz, ~60 phon),
/// clamped ±15. Pure trial-count optimization — recorded per band; the
/// staircase converges to the same PSE from any start.
const BAND_SEEDS: [f64; 9] = [15.0, 8.0, 2.0, -1.0, -3.0, -4.0, 5.0, 11.0, 13.0];

fn octave(center: f64) -> (f64, f64) {
    (center / 2f64.sqrt(), center * 2f64.sqrt())
}

/// Deterministic per-presentation noise seed.
fn mix_seed(session_seed: u64, presentation: u64, interval: u64) -> u64 {
    let mut x = session_seed
        ^ presentation.wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ interval.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x.max(1)
}

// ---------------- the audio side ----------------

/// One interval's recipe: a signal built at MAX_AMP times a multiplier ≤ 1
/// (the multiplier encodes the desired RMS on the normalized axis).
#[derive(Clone, Copy)]
pub struct IntervalSpec {
    pub kind: SignalKind,
    pub mult: f64,
    pub seed: u64,
}

#[derive(Clone)]
pub enum Program {
    /// Level-set: loop this band; gain follows the live atomic.
    LevelLoop { kind: SignalKind, seed: u64 },
    /// One trial: gate/gap/gate, then silence.
    Pair {
        first: IntervalSpec,
        second: IntervalSpec,
    },
}

pub struct ProfileShared {
    pub serial: AtomicU64,
    pub program: Mutex<Program>,
    /// Level-set multiplier in dB relative to the loop signal at MAX_AMP
    /// (f64 bits; always ≤ 0 — attenuation only).
    pub level_gain_bits: AtomicU64,
}

enum SrcState {
    Idle,
    Loop(Box<Signal>),
    Pair {
        sig1: Box<Signal>,
        m1: f64,
        sig2: Box<Signal>,
        m2: f64,
        pos: u64,
    },
}

pub struct ProfileSource {
    shared: Arc<ProfileShared>,
    fs: f64,
    seen: u64,
    state: SrcState,
}

impl ProfileSource {
    pub fn new(shared: Arc<ProfileShared>) -> Self {
        ProfileSource {
            shared,
            fs: 48000.0,
            seen: 0,
            state: SrcState::Idle,
        }
    }
}

/// Raised-cosine gate inside one interval.
fn gate(pos_in_interval: f64, interval: f64, gate_len: f64) -> f64 {
    if pos_in_interval < gate_len {
        0.5 - 0.5 * (std::f64::consts::PI * pos_in_interval / gate_len).cos()
    } else if pos_in_interval > interval - gate_len {
        let t = (interval - pos_in_interval) / gate_len;
        0.5 - 0.5 * (std::f64::consts::PI * t).cos()
    } else {
        1.0
    }
}

impl Source for ProfileSource {
    fn set_rate(&mut self, fs: f64) {
        self.fs = fs;
    }

    fn fill(&mut self, buf: &mut [f64]) -> bool {
        let serial = self.shared.serial.load(Ordering::Relaxed);
        if serial != self.seen {
            self.seen = serial;
            let program = self.shared.program.lock().unwrap().clone();
            self.state = match program {
                Program::LevelLoop { kind, seed } => SrcState::Loop(Box::new(Signal::new(
                    kind,
                    signal::DEFAULT_AMP,
                    self.fs,
                    seed,
                ))),
                Program::Pair { first, second } => SrcState::Pair {
                    sig1: Box::new(Signal::new(
                        first.kind,
                        signal::DEFAULT_AMP,
                        self.fs,
                        first.seed,
                    )),
                    m1: first.mult,
                    sig2: Box::new(Signal::new(
                        second.kind,
                        signal::DEFAULT_AMP,
                        self.fs,
                        second.seed,
                    )),
                    m2: second.mult,
                    pos: 0,
                },
            };
        }
        let interval = (INTERVAL_S * self.fs) as u64;
        let gap = (GAP_S * self.fs) as u64;
        for frame in buf.chunks_exact_mut(2) {
            let s = match &mut self.state {
                SrcState::Idle => 0.0,
                SrcState::Loop(sig) => {
                    let g = 10f64.powf(
                        f64::from_bits(self.shared.level_gain_bits.load(Ordering::Relaxed)) / 20.0,
                    );
                    sig.next_sample() * g
                }
                SrcState::Pair {
                    sig1,
                    m1,
                    sig2,
                    m2,
                    pos,
                } => {
                    let p = *pos;
                    *pos += 1;
                    if p < interval {
                        sig1.next_sample() * *m1 * gate(p as f64 / self.fs, INTERVAL_S, GATE_S)
                    } else if p < interval + gap {
                        // The generator still advances during the gap so the
                        // second interval never re-hears warm-up transients.
                        0.0
                    } else if p < interval * 2 + gap {
                        let pi = (p - interval - gap) as f64 / self.fs;
                        sig2.next_sample() * *m2 * gate(pi, INTERVAL_S, GATE_S)
                    } else {
                        self.state = SrcState::Idle;
                        0.0
                    }
                }
            };
            frame[0] = s;
            frame[1] = s;
        }
        true
    }
}

// ---------------- the session ----------------

pub struct Band {
    pub lo_hz: f64,
    pub hi_hz: f64,
    pub center_hz: f64,
    pub rms_norm_db: f64,
    pub ceil_db: f64,
    pub floor_db: f64,
    pub seed_db: f64,
    /// [start above seed, start below seed]
    pub stairs: [Staircase; 2],
}

pub struct CurrentTrial {
    /// (band index, staircase index) — None for catch trials.
    pub target: Option<(usize, usize)>,
    pub catch_kind: Option<&'static str>, // "lapse" | "order"
    pub offset_db: f64,
    pub anchor_first: bool,
    pub seed_first: u64,
    pub seed_second: u64,
    pub replays: u32,
}

pub struct ProfileSession {
    pub id: String,
    pub headphone: String,
    pub started_ms: u64,
    pub session_seed: u64,
    pub rng: Xorshift,
    pub rate: u32,
    pub device: String,
    pub anchor_rms_norm_db: f64,
    /// Accepted anchor level (None until the level-set step finishes).
    pub anchor_dbfs: Option<f64>,
    pub bands: Vec<Band>,
    pub journal: Vec<serde_json::Value>,
    pub presentations: u64,
    pub answered: u64,
    pub sweep: Vec<(usize, usize)>,
    pub catch_slot: u64,
    pub next_catch_is_lapse: bool,
    pub lapse_total: u64,
    pub lapse_missed: u64,
    pub order_total: u64,
    pub order_chose_second: u64,
    pub current: Option<CurrentTrial>,
    pub resumes: u32,
    pub result_viewed_mid_session: bool,
    pub complete: bool,
    pub shared: Arc<ProfileShared>,
    pub stream_stop: Arc<AtomicBool>,
    pub stream_done: Arc<AtomicBool>,
}

impl ProfileSession {
    pub fn anchor_kind(&self) -> SignalKind {
        let (lo, hi) = octave(ANCHOR_HZ);
        SignalKind::BandNoise {
            lo_hz: lo,
            hi_hz: hi,
        }
    }

    fn band_kind(b: &Band) -> SignalKind {
        SignalKind::BandNoise {
            lo_hz: b.lo_hz,
            hi_hz: b.hi_hz,
        }
    }

    /// RMS of the generated (DEFAULT_AMP) signal on the normalized axis.
    fn rms_at_gen(rms_norm_db: f64) -> f64 {
        rms_norm_db + 20.0 * signal::DEFAULT_AMP.log10()
    }

    /// Multiplier presenting `desired_dbfs` RMS for a signal built at
    /// DEFAULT_AMP whose amp-1 RMS is `rms_norm_db`. May exceed 1 (the
    /// generation amp is clamp-free-quiet); the MAX_RMS_DBFS ceiling bounds
    /// the true peak instead.
    fn mult_for(rms_norm_db: f64, desired_dbfs: f64) -> f64 {
        10f64.powf((desired_dbfs.min(MAX_RMS_DBFS) - Self::rms_at_gen(rms_norm_db)) / 20.0)
    }

    pub fn new(headphone: String, rate: u32, device: String) -> Self {
        let now = crate::now_ms();
        let session_seed = now | 1;
        let (alo, ahi) = octave(ANCHOR_HZ);
        let anchor_rms_norm_db = signal::rms_dbfs(
            SignalKind::BandNoise {
                lo_hz: alo,
                hi_hz: ahi,
            },
            rate as f64,
            7,
            1.0,
        );
        let bands = BAND_CENTERS
            .iter()
            .zip(BAND_SEEDS.iter())
            .map(|(&c, &seed)| {
                let (lo, hi) = octave(c);
                let rms_norm_db = signal::rms_dbfs(
                    SignalKind::BandNoise {
                        lo_hz: lo,
                        hi_hz: hi,
                    },
                    rate as f64,
                    7,
                    1.0,
                );
                Band {
                    lo_hz: lo,
                    hi_hz: hi,
                    center_hz: c,
                    rms_norm_db,
                    ceil_db: EXCURSION_DB,
                    floor_db: -EXCURSION_DB,
                    seed_db: seed,
                    // Placeholder staircases; rebuilt at accept_level once
                    // the headroom-limited clamps are known.
                    stairs: [
                        Staircase::new(default_cfg(seed + 8.0, (-EXCURSION_DB, EXCURSION_DB))),
                        Staircase::new(default_cfg(seed - 8.0, (-EXCURSION_DB, EXCURSION_DB))),
                    ],
                }
            })
            .collect();
        let shared = Arc::new(ProfileShared {
            serial: AtomicU64::new(0),
            program: Mutex::new(Program::LevelLoop {
                kind: SignalKind::BandNoise {
                    lo_hz: alo,
                    hi_hz: ahi,
                },
                seed: 7,
            }),
            level_gain_bits: AtomicU64::new(0),
        });
        let mut s = ProfileSession {
            id: format!("profile-{now}"),
            headphone,
            started_ms: now,
            session_seed,
            rng: Xorshift::new(session_seed),
            rate,
            device,
            anchor_rms_norm_db,
            anchor_dbfs: None,
            bands,
            journal: Vec::new(),
            presentations: 0,
            answered: 0,
            sweep: Vec::new(),
            catch_slot: 0,
            next_catch_is_lapse: true,
            lapse_total: 0,
            lapse_missed: 0,
            order_total: 0,
            order_chose_second: 0,
            current: None,
            resumes: 0,
            result_viewed_mid_session: false,
            complete: false,
            shared,
            stream_stop: Arc::new(AtomicBool::new(false)),
            stream_done: Arc::new(AtomicBool::new(false)),
        };
        s.set_level_dbfs(LEVEL_START_DBFS);
        s.catch_slot = s.rng.next_u64() % CATCH_EVERY;
        s
    }

    pub fn level_dbfs(&self) -> f64 {
        f64::from_bits(self.shared.level_gain_bits.load(Ordering::Relaxed))
            + Self::rms_at_gen(self.anchor_rms_norm_db)
    }

    pub fn set_level_dbfs(&self, dbfs: f64) {
        let gain = dbfs.clamp(-80.0, MAX_RMS_DBFS) - Self::rms_at_gen(self.anchor_rms_norm_db);
        self.shared
            .level_gain_bits
            .store(gain.to_bits(), Ordering::Relaxed);
    }

    /// Fix the anchor level, compute per-band headroom clamps, rebuild the
    /// staircases with real bounds, and present the first trial.
    pub fn accept_level(&mut self) {
        let anchor = self.level_dbfs();
        self.anchor_dbfs = Some(anchor);
        for b in &mut self.bands {
            let ceil = (MAX_RMS_DBFS - anchor).min(EXCURSION_DB);
            b.ceil_db = ceil;
            b.floor_db = -EXCURSION_DB;
            let seed = b.seed_db.clamp(b.floor_db, ceil);
            b.stairs = [
                Staircase::new(default_cfg(seed + 8.0, (b.floor_db, ceil))),
                Staircase::new(default_cfg(seed - 8.0, (b.floor_db, ceil))),
            ];
        }
        self.next_trial();
    }

    fn refill_sweep(&mut self) {
        let mut open: Vec<(usize, usize)> = Vec::new();
        for (bi, b) in self.bands.iter().enumerate() {
            for (si, st) in b.stairs.iter().enumerate() {
                if st.done().is_none() {
                    open.push((bi, si));
                }
            }
        }
        // Fisher–Yates with the session RNG.
        for i in (1..open.len()).rev() {
            let j = (self.rng.next_u64() % (i as u64 + 1)) as usize;
            open.swap(i, j);
        }
        self.sweep = open;
    }

    /// Schedule and present the next pair (or mark the session complete).
    pub fn next_trial(&mut self) {
        let anchor_dbfs = match self.anchor_dbfs {
            Some(a) => a,
            None => return,
        };
        self.presentations += 1;
        let seed_first = mix_seed(self.session_seed, self.presentations, 0);
        let seed_second = mix_seed(self.session_seed, self.presentations, 1);
        let anchor_kind = self.anchor_kind();
        let anchor_norm = self.anchor_rms_norm_db;

        // Catch slot (jittered inside each CATCH_EVERY block)?
        if self.presentations % CATCH_EVERY == self.catch_slot {
            self.catch_slot = self.rng.next_u64() % CATCH_EVERY;
            let is_lapse = self.next_catch_is_lapse;
            self.next_catch_is_lapse = !is_lapse;
            let louder_first = self.rng.next_bool();
            let delta = if is_lapse { LAPSE_DELTA_DB } else { 0.0 };
            let (l1, l2) = if louder_first {
                (anchor_dbfs + delta, anchor_dbfs)
            } else {
                (anchor_dbfs, anchor_dbfs + delta)
            };
            *self.shared.program.lock().unwrap() = Program::Pair {
                first: IntervalSpec {
                    kind: anchor_kind,
                    mult: Self::mult_for(anchor_norm, l1),
                    seed: seed_first,
                },
                second: IntervalSpec {
                    kind: anchor_kind,
                    mult: Self::mult_for(anchor_norm, l2),
                    seed: seed_second,
                },
            };
            self.shared.serial.fetch_add(1, Ordering::Relaxed);
            self.current = Some(CurrentTrial {
                target: None,
                catch_kind: Some(if is_lapse { "lapse" } else { "order" }),
                offset_db: delta,
                anchor_first: louder_first, // for lapse: louder side first
                seed_first,
                seed_second,
                replays: 0,
            });
            return;
        }

        if self.sweep.is_empty() {
            self.refill_sweep();
        }
        let Some((bi, si)) = self.sweep.pop() else {
            // Every staircase is finished — the session is complete.
            self.complete = true;
            self.current = None;
            *self.shared.program.lock().unwrap() = Program::LevelLoop {
                kind: anchor_kind,
                seed: 7,
            };
            // Leave the source idle by not bumping the serial.
            return;
        };
        let b = &self.bands[bi];
        let offset = b.stairs[si].level_db();
        let anchor_first = self.rng.next_bool();
        let band_spec = IntervalSpec {
            kind: Self::band_kind(b),
            mult: Self::mult_for(b.rms_norm_db, anchor_dbfs + offset),
            seed: if anchor_first {
                seed_second
            } else {
                seed_first
            },
        };
        let anchor_spec = IntervalSpec {
            kind: anchor_kind,
            mult: Self::mult_for(anchor_norm, anchor_dbfs),
            seed: if anchor_first {
                seed_first
            } else {
                seed_second
            },
        };
        let (first, second) = if anchor_first {
            (anchor_spec, band_spec)
        } else {
            (band_spec, anchor_spec)
        };
        *self.shared.program.lock().unwrap() = Program::Pair { first, second };
        self.shared.serial.fetch_add(1, Ordering::Relaxed);
        self.current = Some(CurrentTrial {
            target: Some((bi, si)),
            catch_kind: None,
            offset_db: offset,
            anchor_first,
            seed_first,
            seed_second,
            replays: 0,
        });
    }

    /// Journal + apply one answer, then present the next trial.
    pub fn answer(&mut self, second_louder: bool) -> Result<(), String> {
        let cur = self.current.take().ok_or("no trial is playing")?;
        self.answered += 1;
        let mut entry = serde_json::json!({
            "i": self.presentations,
            "atMs": crate::now_ms() - self.started_ms,
            "offsetDb": cur.offset_db,
            "anchorFirst": cur.anchor_first,
            "seedFirst": cur.seed_first,
            "seedSecond": cur.seed_second,
            "answer": if second_louder { "second" } else { "first" },
            "replays": cur.replays,
        });
        match (cur.target, cur.catch_kind) {
            (Some((bi, si)), _) => {
                let b = &mut self.bands[bi];
                // The comparison band was the non-anchor interval.
                let comparison_louder = if cur.anchor_first {
                    second_louder
                } else {
                    !second_louder
                };
                entry["bandLoHz"] = serde_json::json!(b.lo_hz);
                entry["bandHiHz"] = serde_json::json!(b.hi_hz);
                entry["staircase"] = serde_json::json!(si);
                entry["comparisonJudgedLouder"] = serde_json::json!(comparison_louder);
                b.stairs[si].answer(comparison_louder);
            }
            (None, Some("lapse")) => {
                self.lapse_total += 1;
                // anchor_first here means "the louder interval was first".
                let correct = cur.anchor_first != second_louder;
                if !correct {
                    self.lapse_missed += 1;
                }
                entry["catch"] = serde_json::json!("lapse");
                entry["correct"] = serde_json::json!(correct);
            }
            (None, _) => {
                self.order_total += 1;
                if second_louder {
                    self.order_chose_second += 1;
                }
                entry["catch"] = serde_json::json!("order");
            }
        }
        self.journal.push(entry);
        self.next_trial();
        Ok(())
    }

    pub fn replay(&mut self) {
        if let Some(cur) = &mut self.current {
            cur.replays += 1;
            self.shared.serial.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn lapse_rate(&self) -> f64 {
        if self.lapse_total == 0 {
            0.0
        } else {
            self.lapse_missed as f64 / self.lapse_total as f64
        }
    }

    /// The full record (partial when `complete` is false) — the journal is
    /// the source of truth; band summaries are denormalized.
    pub fn record(&self) -> serde_json::Value {
        let bands: Vec<serde_json::Value> = self
            .bands
            .iter()
            .map(|b| {
                let combined = combine_pse(&b.stairs[0], &b.stairs[1]);
                let ends: Vec<&'static str> = b
                    .stairs
                    .iter()
                    .map(|s| match s.done() {
                        Some(StaircaseEnd::Converged) => "converged",
                        Some(StaircaseEnd::CappedOut) => "cappedOut",
                        Some(StaircaseEnd::Unconverged) => "unconverged",
                        Some(StaircaseEnd::RailedHigh) => "railedHigh",
                        Some(StaircaseEnd::RailedLow) => "railedLow",
                        None => "open",
                    })
                    .collect();
                let bound = b.stairs.iter().find_map(|s| s.bound_db());
                serde_json::json!({
                    "loHz": b.lo_hz, "hiHz": b.hi_hz, "centerHz": b.center_hz,
                    "rmsNormDb": b.rms_norm_db,
                    "ceilDb": b.ceil_db, "floorDb": b.floor_db,
                    "seedDb": b.seed_db,
                    "pseDb": combined.map(|c| c.pse_db),
                    "uncertaintyDb": combined.map(|c| c.uncertainty_db),
                    "disagreementDb": combined.map(|c| c.disagreement_db),
                    "pseBoundDb": bound,
                    "end": ends,
                    "trials": b.stairs.iter().map(|s| s.trials()).sum::<u32>(),
                    "staircases": b.stairs.iter().map(|s| serde_json::json!({
                        "reversals": s.reversals(),
                        "estimateDb": s.estimate_db(),
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        serde_json::json!({
            "schema": 1,
            "kind": "hearingProfile",
            "id": self.id,
            "headphone": self.headphone,
            "startedMs": self.started_ms,
            "finished": self.complete,
            "sessionSeed": self.session_seed,
            "sampleRateHz": self.rate,
            "device": self.device,
            "playback": { "mode": "exclusive", "intervalMs": (INTERVAL_S * 1000.0) as u64,
                          "gapMs": (GAP_S * 1000.0) as u64, "gateMs": (GATE_S * 1000.0) as u64 },
            "anchor": { "centerHz": ANCHOR_HZ, "loHz": octave(ANCHOR_HZ).0, "hiHz": octave(ANCHOR_HZ).1,
                        "rmsNormDb": self.anchor_rms_norm_db,
                        "acceptedRmsDbfs": self.anchor_dbfs },
            "levelValidity": "resultsValidOnlyAtSessionLevel",
            "bands": bands,
            "catch": { "everyNth": CATCH_EVERY,
                       "lapse": { "total": self.lapse_total, "missed": self.lapse_missed, "deltaDb": LAPSE_DELTA_DB },
                       "order": { "total": self.order_total, "choseSecond": self.order_chose_second } },
            "lowReliability": self.lapse_total >= 5 && self.lapse_rate() > 0.10,
            "resultViewedMidSession": self.result_viewed_mid_session,
            "resumes": self.resumes,
            "presentations": self.presentations,
            "journal": self.journal,
        })
    }

    /// Rebuild an unfinished session from its saved record: staircases are
    /// reconstructed by replaying the journal (exact — the core replay test
    /// proves it), catch counters recounted, the RNG reseeded (it only
    /// drives scheduling/order, never statistics validity).
    pub fn resume_from(rec: &serde_json::Value, rate: u32) -> Result<Self, String> {
        let headphone = rec
            .get("headphone")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let device = rec
            .get("device")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let mut s = ProfileSession::new(headphone, rate, device);
        s.id = rec
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or("record has no id")?
            .to_string();
        s.started_ms = rec
            .get("startedMs")
            .and_then(|v| v.as_u64())
            .unwrap_or(s.started_ms);
        s.session_seed = rec
            .get("sessionSeed")
            .and_then(|v| v.as_u64())
            .unwrap_or(s.session_seed);
        s.presentations = rec
            .get("presentations")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        s.resumes = rec.get("resumes").and_then(|v| v.as_u64()).unwrap_or(0) as u32 + 1;
        s.result_viewed_mid_session = rec
            .get("resultViewedMidSession")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        // Fresh scheduling randomness; statistics don't depend on it.
        s.rng = Xorshift::new(mix_seed(s.session_seed, s.presentations + 1, 2));
        s.catch_slot = s.rng.next_u64() % CATCH_EVERY;
        let anchor = rec
            .get("anchor")
            .and_then(|a| a.get("acceptedRmsDbfs"))
            .and_then(|v| v.as_f64());
        if let Some(a) = anchor {
            s.set_level_dbfs(a);
            s.anchor_dbfs = Some(a);
            for b in &mut s.bands {
                let ceil = (MAX_RMS_DBFS - a).min(EXCURSION_DB);
                b.ceil_db = ceil;
                b.floor_db = -EXCURSION_DB;
                let seed = b.seed_db.clamp(b.floor_db, ceil);
                b.stairs = [
                    Staircase::new(default_cfg(seed + 8.0, (b.floor_db, ceil))),
                    Staircase::new(default_cfg(seed - 8.0, (b.floor_db, ceil))),
                ];
            }
        }
        // Replay the journal into the fresh staircases + catch counters.
        let entries = rec
            .get("journal")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        for e in &entries {
            match e.get("catch").and_then(|v| v.as_str()) {
                Some("lapse") => {
                    s.lapse_total += 1;
                    if e.get("correct").and_then(|v| v.as_bool()) == Some(false) {
                        s.lapse_missed += 1;
                    }
                }
                Some(_) => {
                    s.order_total += 1;
                    if e.get("answer").and_then(|v| v.as_str()) == Some("second") {
                        s.order_chose_second += 1;
                    }
                }
                None => {
                    let (lo, si, louder) = (
                        e.get("bandLoHz").and_then(|v| v.as_f64()),
                        e.get("staircase").and_then(|v| v.as_u64()),
                        e.get("comparisonJudgedLouder").and_then(|v| v.as_bool()),
                    );
                    if let (Some(lo), Some(si), Some(louder)) = (lo, si, louder) {
                        if let Some(b) = s.bands.iter_mut().find(|b| (b.lo_hz - lo).abs() < 0.01) {
                            if let Some(st) = b.stairs.get_mut(si as usize) {
                                st.answer(louder);
                            }
                        }
                    }
                }
            }
        }
        s.answered = entries.len() as u64;
        s.journal = entries;
        Ok(s)
    }

    /// Spawn (or respawn) the exclusive stream for this session's shared
    /// state. Runs until `stream_stop`; raises `stream_done` on exit (TB-27).
    pub fn spawn_stream(&mut self, on_error: impl FnOnce(String) + Send + 'static) {
        self.stream_stop = Arc::new(AtomicBool::new(false));
        self.stream_done = Arc::new(AtomicBool::new(false));
        let shared = self.shared.clone();
        let (stop, done) = (self.stream_stop.clone(), self.stream_done.clone());
        std::thread::spawn(move || {
            let mut src = ProfileSource::new(shared);
            let r = playback::play(OutputMode::Exclusive, &mut src, 1.0, &stop, |_| {});
            done.store(true, Ordering::Relaxed);
            if let Err(e) = r {
                if !stop.load(Ordering::Relaxed) {
                    on_error(e.to_string());
                }
            }
        });
    }

    pub fn stop_stream(&self) {
        self.stream_stop.store(true, Ordering::Relaxed);
    }
}

fn default_cfg(start_db: f64, clamp_db: (f64, f64)) -> StaircaseConfig {
    StaircaseConfig {
        start_db,
        initial_step_db: 4.0,
        min_step_db: 1.0,
        reversals_per_halving: 2,
        min_step_reversals: 6,
        max_trials: 40,
        clamp_db,
    }
}

/// One session at a time (ABX-static pattern).
pub static PROFILE: Mutex<Option<ProfileSession>> = Mutex::new(None);
