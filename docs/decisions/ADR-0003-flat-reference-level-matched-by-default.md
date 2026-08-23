# ADR-0003: A flat reference always exists, and level matching is on by default

- **Status:** Accepted
- **Date:** 2026-08-22
- **Source:** Idea-mapping session (user: "have a stock preset which can be level matched to everything"; confirmed, noting the app name "makes perfect sense too")

## Context
Louder reliably sounds better, even at fractions of a dB — so an unmatched EQ on/off toggle flatters every preset. Most user opinions form in the casual "toggle and listen" moment, not in formal tests; if only test mode matches levels, the product lies exactly where it matters most.

## Decision
A true-bypass flat reference preset (0 preamp, no filters — see TB-17) always exists and cannot be deleted. Every preset is automatically loudness-matched (LUFS-based) against that reference by default — in casual toggling and preset switching, not just in tests. Unmatched comparison is an explicit, visibly-flagged opt-out.

## Why
Honesty-by-default is the product's identity — the namesake is the Fletcher–Munson research showing loudness changes perceived frequency balance. Rejected: matching only inside test mode (leaves the highest-traffic comparison dishonest); making matching opt-in (defaults are destiny).

## Consequences
- Fletcher needs a matching-gain estimate even with no specific track playing (system-wide mode): computable from the preset's response curve against a reference spectrum. Estimation method is a residual design question (Q-06). Track mode measures true LUFS directly.
- The flat reference's integrity must be guarded (TB-17); TB-08 (matching hits the rails) needs defined behavior.
- Preset "gain" and preset "matching offset" are separate concepts in the data model.
