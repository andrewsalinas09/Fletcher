# Roadmap

Scope authority (ADR-0006): everything in FEATURES.md ships; phases order it by importance and dependency. Every phase ends in something usable.

## Current focus

**Phase 2, mid-flight (2026-08-23).** Done: curve/strip editing (+multi-select), presets (AutoEQ import, Peace import, rename, filter clipboard), global reference loudness, blind ABX with stats + labeled replay (first result 13/16 p=0.011), undo graph (canvas, pop-out, persistence, export, rail), Q-24 history inspector (parametric + spectral diffs, any-N-node compare, listen previews, node-vs-node ABX — Q-17 closed, edge weights, notes/pins). **Next (user-sequenced 2026-08-23):** ① **Settings tab v1** from the approved artboard (filter ordering, level-matching honesty switch, reference-loudness placeholder, autostart, Standard/Advanced mode, APO status). ② **Clip Studio** including the shared **signal generator** (ADR-0004; one generator, never duplicated — it then powers Settings' reference calibration (Q-16) and the Lab's test material). ③ **Listening Lab expansion after Clip Studio** (clips in tests, adaptive/sequential ABX, preference voting). Still folded forward: normative VISION rewrite, fingerprint-format sketch (Q-15).

## Phases

- **Phase 0 — Design.** ✔ Closed 2026-08-22 (VISION rewrite + fingerprint sketch folded into Phase 1).
- **Phase 1 — Skeleton.** ✔ Exit criterion met 2026-08-23: a stranger can A/B two configurations with a global hotkey — level-matched. Shipped beyond plan: live curve editing, presets with Peace import, undo, tooltips, tray. Residual to fold forward: VISION rewrite, fingerprint-format sketch, Settings tab.
- **Phase 2 — The honest EQ.** *(← we are here)* Tauri 2 scaffold (ADR-0005); config engine with lossless round-trip tests (Q-09); device layer + APO detection/guidance (TB-01); tray + global hotkey; sighted system-wide A/B. Exit: a stranger can A/B two presets with a hotkey.
- **Phase 2 — The honest EQ.** Curve editor (filters vs. summed response); presets; AutoEQ import (Q-08); auto-preamp (TB-06); flat reference + app-wide level matching (ADR-0003); blind ABX system-wide with statistics (Q-05). Exit: the first ten minutes from FEATURES.md work end-to-end.
- **Phase 3 — The Listening Lab.** In-app track engine (decode → biquad bus → crossfade → WASAPI, Q-04); Clip Studio + libraries (ADR-0004); clip-battery discrimination and preference testing; tuning×testing fusion (Q-14); live spectrograms/waveforms incl. flat-vs-EQ side-by-side. Exit: import a song, build clips, run a battery, read real statistics.
- **Phase 4 — The Fingerprint Lab.** Measurement engine (sweeps + synchronized capture, reseating statistics); guided fingerprint capture; personal library; headphone-to-headphone matching with re-measure verification (Q-12, TB-14, TB-16). Exit: two of your headphones, statistically matched, verified.
- **Phase 5 — The ecosystem.** Fingerprint export/import + bridging with per-frequency confidence (Q-15, TB-18/19); community features as scoped by Q-10; distribution polish — code signing / SmartScreen, winget, auto-update (explicitly deferred by the user 2026-08-22: "pushed back for now"). Exit: import a reviewer's library, audition a headphone you don't own, see honest confidence bands.

Cross-cutting from Phase 1 onward: everything recorded — full provenance on every test, measurement, and comparison (ADR-0006).
