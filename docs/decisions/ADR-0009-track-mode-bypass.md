# ADR-0009: Track mode bypasses APO via exclusive mode by default, config-blank as a selectable second mode

- **Status:** Accepted
- **Date:** 2026-08-22
- **Source:** Q-04 spike (verified live: exclusive bypasses APO on Schiit Modi 3+) + user: "yeah i agree. You want it to take over — but have both modes!"

## Context
Track-mode playback through the normal shared path gets double-processed by APO (TB-03). The spike proved WASAPI exclusive mode bypasses APO cleanly, at two costs: it evicts other audio apps ungracefully (Apple Music errored, no auto-recover) and bypasses Windows volume (raw DAC level out). The alternative — temporarily blanking the device's APO config — keeps shared mode and Windows volume but mutates user config (TB-11 crash-restore) and races Peace's file watcher.

## Decision
Both mechanisms ship as user-selectable modes. **Exclusive mode is the default**: Fletcher takes the device over for the session, with a pre-session warning that other audio will stop and may need manual restart. **Config-blank mode** is the second option (and automatic fallback when exclusive init fails): journaled, crash-proof blank/restore of the device's APO config while playing shared.

## Why
Exclusive as default: zero config mutation, no restore hazard, no watcher races, bit-clean path — and silencing competing audio during a blind listening test is correct behavior. Config-blank as a real mode, not just a fallback: some devices/drivers refuse exclusive, some users need Windows volume or background audio to keep working. Rejected: exclusive-only (strands those users), config-blank-only (carries TB-11 risk for everyone and leaves system sounds bleeding into tests).

## Consequences
- The track engine owns its gain stage in both modes; in exclusive it is the *only* volume control, so it starts conservative and ramps (TB-20).
- Config-blank mode must implement journaled restore (TB-11) and tolerate Peace rewriting files mid-session (TB-09).
- Mode choice is per-session, remembered, surfaced per ADR-0002 (advanced users see both; the default just works).
- Exclusive format negotiation (already probed: f32/24i/16i fallback ladder) becomes engine code.
