# ADR-0006: Scope by sequencing, not cutting

- **Status:** Accepted
- **Date:** 2026-08-22
- **Source:** Idea-mapping session (user: "everything is going to be built, we just need to plan it properly and architect it all into phases, the most important parts up front first")

## Context
The feature map (FEATURES.md) is large — core EQ, the Listening Lab, Clip Studio, the Fingerprint Lab, bridging/ecosystem. The conventional move is to cut to an MVP. The user's direction is explicit: nothing is cut; planning means ordering.

## Decision
Every feature in FEATURES.md is committed scope. The roadmap orders work by importance and dependency — most important parts first — and every phase ends in a usable, shippable increment. "Should we drop X?" is not a live question; "which phase is X in?" is.

## Why
This is a passion project where the full vision *is* the motivation, and the doc system exists precisely to make a large scope tractable across many sessions. The features also compound (clips feed tests feed stats; fingerprints feed matching feed bridging) — cutting one weakens the others. Rejected: MVP-minimalism (optimizes for a launch this project doesn't need).

## Consequences
- Architecture must anticipate late phases now — e.g. the audio engine is designed knowing the measurement engine (sweep playback + synchronized capture) will sit on it; the fingerprint format is designed before the Lab ships (Q-15).
- ROADMAP.md is the scope authority: phases, not feature debates.
- Timeline is elastic by choice; the fixed thing is direction.
- Corollary, from the same session: **everything is recorded** — every test, measurement, and comparison stores full provenance (configs, levels, matching offsets, capture metadata), so results are always auditable. "Everything recorded so everything is known."
