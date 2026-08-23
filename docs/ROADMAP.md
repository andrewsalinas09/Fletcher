# Roadmap

Scope authority (ADR-0006): everything in FEATURES.md ships; phases order it by importance and dependency. Every phase ends in something usable.

## Current focus

**Phase 1 — skeleton.** Phase 0 closed 2026-08-22: nine ADRs, both spikes verified live (hot-reload "basically 0 latency"; exclusive-mode bypass confirmed → ADR-0009). Next up: restructure to the Tauri 2 scaffold, config engine with lossless round-trip tests (first fixture: the live Peace HD650 preset), device layer, tray + hotkey, sighted system-wide A/B. Folded in from Phase 0: normative VISION rewrite, fingerprint-format sketch (Q-15).

## Phases

- **Phase 0 — Design.** ✔ Closed 2026-08-22 (VISION rewrite + fingerprint sketch folded into Phase 1).
- **Phase 1 — Skeleton.** *(← we are here)* Tauri 2 scaffold (ADR-0005); config engine with lossless round-trip tests (Q-09); device layer + APO detection/guidance (TB-01); tray + global hotkey; sighted system-wide A/B. Exit: a stranger can A/B two presets with a hotkey.
- **Phase 2 — The honest EQ.** Curve editor (filters vs. summed response); presets; AutoEQ import (Q-08); auto-preamp (TB-06); flat reference + app-wide level matching (ADR-0003); blind ABX system-wide with statistics (Q-05). Exit: the first ten minutes from FEATURES.md work end-to-end.
- **Phase 3 — The Listening Lab.** In-app track engine (decode → biquad bus → crossfade → WASAPI, Q-04); Clip Studio + libraries (ADR-0004); clip-battery discrimination and preference testing; tuning×testing fusion (Q-14); live spectrograms/waveforms incl. flat-vs-EQ side-by-side. Exit: import a song, build clips, run a battery, read real statistics.
- **Phase 4 — The Fingerprint Lab.** Measurement engine (sweeps + synchronized capture, reseating statistics); guided fingerprint capture; personal library; headphone-to-headphone matching with re-measure verification (Q-12, TB-14, TB-16). Exit: two of your headphones, statistically matched, verified.
- **Phase 5 — The ecosystem.** Fingerprint export/import + bridging with per-frequency confidence (Q-15, TB-18/19); community features as scoped by Q-10; distribution polish — code signing / SmartScreen, winget, auto-update (explicitly deferred by the user 2026-08-22: "pushed back for now"). Exit: import a reviewer's library, audition a headphone you don't own, see honest confidence bands.

Cross-cutting from Phase 1 onward: everything recorded — full provenance on every test, measurement, and comparison (ADR-0006).
