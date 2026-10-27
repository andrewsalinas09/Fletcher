# ADR-0014: Output trim is an offset on the matched preamp, never an override

- **Status:** Accepted
- **Date:** 2026-08-29
- **Source:** Live failure on the dev machine (TB-30/TB-31) — user could not make the app louder

## Context
Fletcher's written preamp is *derived*: `matched_preamp` normalizes every chain, preset and A/B side to the global reference loudness, tightened to clip-safe (ADR-0003, Q-06). Nothing exposed a raw level control, and `reference_db` is capped at 0 dB by design.

That cap is correct for its own job and useless for the one the user actually hit. On 2026-08-29 a hand-added `Preamp: -10 dB` sat in `config.txt` — a line Fletcher may not touch (ADR-0007) — stacking with fletcher.txt's own −10.9 for −20.9 dB total. The app was quiet, the number was visible, and there was **no lever anywhere in the UI** to answer it: reference could only go down, and the offending line was off-limits.

## Decision
A persisted **output trim** (`preampTrimDb`, −20…+12 dB) is added to the matched preamp inside `matched_preamp_full` — the single function every write path reaches the file through. It is the only lever that may push the written preamp above 0 dB. It is an offset; the matched value keeps being computed exactly as before (`matched_preamp_untrimmed`), and `shortfall` is untouched.

Settings also *reports* the foreign attenuation it cannot edit (`foreignPreampDb`), and the EQ room shows a chip when it is non-zero.

## Why
Replacing the matched preamp with a user number would make every A/B a louder-vs-quieter comparison and quietly void the honesty guarantee the whole Listening Lab rests on. A constant added to **both sides** cancels in any comparison: absolute loudness moves, every *relative* guarantee survives. That is the only form of level control that composes with ADR-0003, so it is the form that ships.

Reporting the foreign preamp follows the same principle applied to the UI: Fletcher may not edit those lines, but silently presenting a quiet chain with no explanation is the same class of lie as TB-30's un-wired strip.

Rejected: raising `reference_db`'s cap (breaks matching — a boosted chain can only be matched if its peaks fit under 0 dBFS); editing the foreign line (violates ADR-0007's blast-radius rule, and it is exactly the line another tool may own).

## Consequences
- 1 dB steps in the UI, never a jump — TB-20 extends to any control that moves absolute level.
- `set_preamp_trim` carries `set_reference_db`'s guards: refused during a blind trial (TB-26) or a hearing-profile session (TB-29).
- Above 0 dB total the clip-safe cap has been deliberately overridden by the user; the UI says so rather than silently re-clamping.
- Presets store the untrimmed matched preamp, so the trim stays a playback-side property and presets remain portable.
