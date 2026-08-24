# ADR-0013 · The mic-free hearing profile: procedure, statistics, and the honesty stance

**Status:** Accepted 2026-08-23 (user chose it as the Fingerprints tab's first resident; design research-grounded per user directive)

## Context

Q-19 sketched estimating the listener's perceived frequency response with no
hardware, by loudness-matching noise bands. The procedure design was grounded
in the actual literature behind the modern ISO 226 contours (Takeshima,
Suzuki et al.): direct comparison against one fixed reference presented as
timed pairs with randomized order, adaptively leveled — never adjacent-band
chaining (error compounds per hop; the 1956 contours had to be re-measured
partly over procedure bias).

## Decision

**Task**: 2I-2AFC timed pairs — 800 ms octave-noise intervals, 300 ms gap,
50 ms raised-cosine gates (uniform envelopes: no timbral tell; 20 ms would
thump at 63 Hz), anchor position randomized per trial, question "which was
LOUDER — not brighter, not fuller". R replays the identical pair (recorded).
Fresh deterministic noise seeds per trial from the session seed.

**Topology**: every band directly vs a fixed **500 Hz octave anchor**
(353.6–707.1 Hz — below the seating-variance region, above bass rolloff, the
flat part of both ISO 226 and typical headphones). Bands v1: octave-wide at
63…16k (9 comparisons), keyed by (loHz, hiHz) so refinement bands are
additive later.

**Estimator**: two bracketing **1-up-1-down staircases** per band (converges
at the 50% point = the PSE; 2-down-1-up targets detection thresholds — wrong
tool), starting ±8 dB around an ISO-shaped seed (recorded; trial-count
optimization only). Steps 4→2→1 dB, stop at 6 min-step reversals, cap 40
trials. Band PSE = mean of the two; uncertainty = max(|Δ|/2, s_pooled/√(n/2),
0.5 dB) — a display bound that never understates. (RMLSP, the literature's
ML procedure, is the recorded v2 alternative; with bracketed dual starts and
~18-way interleave the bias protections are equivalent and the pure state
machine gets exact-value tests.)

**Levels**: exclusive-only (nothing colors the measurement — not APO, not
Fletcher, not Peace; fail-fast if refused). One stream per block with
in-signal gating (the engine's mandatory per-stream ramp would otherwise
shape interval 1 differently — a TB-12-class asymmetry). All offsets on a
normalized-RMS axis (`signal::rms_dbfs`; band RMS scales with √bandwidth).
Session level chosen once by the listener from −70 dBFS (TB-20), recorded;
staircase excursion ±20 dB, headroom-capped per band; two pushes against a
clamp = **railed → PSE censored** ("at least this"), drawn as an arrow,
never fudged.

**Reliability**: ~11% catch trials, anchor-vs-anchor only — lapse catches
(+10 dB; >10% miss rate flags the session lowReliability, displayed never
discarded) and order catches (0 dB; measures the time-order error, reported
not corrected). Interleaved scheduling across all staircases (untrackable,
fatigue spread, partial sessions cover every band). Everything journaled;
partial records persist per answer (crash-safe resume; staircases rebuild by
journal replay — proven by a core test).

**Repeats + master**: sessions repeat freely, stay separate; a per-headphone
MASTER curve is computed at read time (inverse-variance weighted mean;
between-session spread widens the bars honestly; railed entries never
average).

**The honesty stance (TB-23, a product decision)**: v1 is display + record
only. The result screen says, verbatim in spirit: it is a description of
your hearing at one loudness, NOT an EQ target; the contour shape is normal
hearing, flattening it would sound wrong for everyone; results don't
transfer across volumes; ears+headphone+session are inseparable; not a
medical hearing test. No apply/export-to-filter affordance exists — not even
greyed (ADR-0002 applies to features that exist; this one deliberately
doesn't). No absolute dB SPL, no medical vocabulary.

## Consequences

The Fingerprints tab un-greys with the profile as its first resident. New
disk area `%APPDATA%\Fletcher\profiles\`. `stats.rs` gains the Staircase
machinery; `signal.rs` the RMS normalization; `profile.rs` owns the session.
Known risks recorded in Q-19's resolution: time-order bias (measured, not
corrected), loudness-vs-timbre confusion (inflates displayed uncertainty),
headphone distortion at high low-band levels (inseparable without a mic —
lives in the caveat), interesting listeners rail at 63 Hz (the anchor-level
warning is the only lever).
