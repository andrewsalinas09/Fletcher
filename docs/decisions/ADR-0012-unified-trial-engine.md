# ADR-0012 · The unified trial engine: one blind-test core across protocol, material, and stop rule

**Status:** Accepted 2026-08-23 (plan approved by the user; scope ruling: clip/battery tests + preference voting + adaptive ABX + Lab redesign, contenders any-vs-any incl. history nodes)

## Context

The original ABX engine was one fixed shape: active preset vs flat, over system
audio, for a fixed trial count. Phase 3 needs blind trials over clip batteries
(waking ADR-0011's dormant "testing" play method), a preference protocol
(TB-15), and honest sequential stopping — without growing three parallel test
engines.

## Decision

One server-side session (`TrialSession`, the `TRIAL` static — the renamed ABX
static) with three orthogonal axes:

**Protocol — `Abx | Pref`.** One assignments vector, one meaning per protocol:
ABX "X is A per trial"; Pref "slot 1 is A per trial". Preference auditions are
`1`/`2` only (an open A/B audition would unblind the slot), there is no
"correct", and the result carries `preferredA` + an exact **two-sided**
binomial p — presentation is visibly different from ABX (closes TB-15).

**Material — `System | Clips`.** System is the classic config-swap path,
untouched. Clips wakes the dual-bus testing mode: **exclusive only, fail
fast** (shared would double-process through APO — TB-03); both contender
chains run in-engine every frame (`BusConfig.specs_b`; TB-12 symmetric
buffering; the 15 ms equal-power crossfade is the switch); each trial solos
one clip on loop; per-clip **true-LUFS trims** measured at start over the clip
region (positive trims capped at post-chain peak headroom − 0.3 dB, shortfall
recorded — TB-06); per-track stream gain toward the reference target,
attenuate-only. Clip order is **shuffled then grouped by track**: the blind
protects the assignment (iid per trial), so material order buys nothing
statistically, and grouping minimizes exclusive open/close churn. While a clip
trial runs, `TRIAL_OWNS_TRACK` gates every public track command and live chain
push — the trial owns the engine; ordinary playback and system-material
sessions still refuse each other. Device loss suspends (answers survive);
exits are resume / finish-early (recorded, flagged) / cancel. Lock order where
both are held: TRIAL → TRACK.

**Stop rule — `FixedN | Sprt`.** Adaptive ABX is Wald's SPRT: H0 p = 0.5 vs
H1 p = 0.75 (the 2AFC threshold-detection convention), α = β = 0.05, cap 40
trials — declared in the record before the first vote. The server decides and
auto-finishes, so bounds-vs-score never reaches the UI mid-session (TB-24
stays clean). The recorded verdict is the SPRT decision; the exact binomial p
at the stop is recorded flagged `sequential` and presented as nominal, never
as the primary criterion. **Preference is fixed-N only**: optional stopping
biases the apparent preference strength upward.

Session JSON is **additive only** (`protocol`, `material`, `stopRule`,
`sequential`, `decision`, `finishedEarly`, `playback`, `trims`, per-trial
`clipId`) — old records parse untouched, and per-clip/per-tag breakdowns come
from joining `log[].clipId` against clip tags at read time, never stored.
Batteries live in SQLite (`batteries`, `battery_clips`); names are
denormalized into session records so provenance survives deletion (ADR-0006).

## Consequences

`abx_*` commands remain as working shims over the unified engine. The Lab's
UI (LabHome/TrialRoom artboards) exposes the three axes as the NEW TEST rail's
decisions. The dormant dual-bus machinery is no longer dormant; TB-12's
in-engine claim is now testable by ear. Statistics live in fletcher-core
(`binomial_p_two_sided`, `sprt_step`, `sprt_bounds_at`) with exact-value
tests. Rejected: a second engine per protocol (drift between them would be a
blinding bug factory), shared-mode clip trials (TB-03), and client-side stop
decisions (information leak, TB-24).
