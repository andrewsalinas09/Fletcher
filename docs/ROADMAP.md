# Roadmap

Scope authority (ADR-0006): everything in FEATURES.md ships; phases order it by importance and dependency. Every phase ends in something usable.

## Current focus

**Phase 3, mid-flight (2026-08-23 night).** Done through Phase 2 + most of 3: curve/strip editing, presets, global reference loudness, undo graph + Q-24 history inspector, Settings v1, **Clip Studio** (track engine ADR-0009/0011, NLE timeline, clips/moments, scopes + satellites, signal generator incl. Q-25 mix/modulators/JSON recipes, yt-dlp import), and the **Listening Lab expansion (ADR-0012)**: unified trial engine — clip/battery blind trials through the exclusive dual-bus testing mode, preference voting (TB-15 closed), adaptive SPRT ABX, any-vs-any contenders incl. history nodes, the finder + batteries, per-tag breakdowns, redesigned room per the LabHome/TrialRoom artboards. **Awaiting live user verification:** clip-trial audio path (TB-12 by ear), preference + adaptive flows. **Next:** spectrogram box-select moments (Q-18 capture half), tuning×testing fusion (Q-14), M8 residual docs. Still folded forward: normative VISION rewrite, fingerprint-format sketch (Q-15).

## Phases

- **Phase 0 — Design.** ✔ Closed 2026-08-22 (VISION rewrite + fingerprint sketch folded into Phase 1).
- **Phase 1 — Skeleton.** ✔ Exit criterion met 2026-08-23: a stranger can A/B two configurations with a global hotkey — level-matched. Shipped beyond plan: live curve editing, presets with Peace import, undo, tooltips, tray. Residual to fold forward: VISION rewrite, fingerprint-format sketch, Settings tab.
- **Phase 2 — The honest EQ.** *(← we are here)* Tauri 2 scaffold (ADR-0005); config engine with lossless round-trip tests (Q-09); device layer + APO detection/guidance (TB-01); tray + global hotkey; sighted system-wide A/B. Exit: a stranger can A/B two presets with a hotkey.
- **Phase 2 — The honest EQ.** Curve editor (filters vs. summed response); presets; AutoEQ import (Q-08); auto-preamp (TB-06); flat reference + app-wide level matching (ADR-0003); blind ABX system-wide with statistics (Q-05). Exit: the first ten minutes from FEATURES.md work end-to-end.
- **Phase 3 — The Listening Lab.** In-app track engine (decode → biquad bus → crossfade → WASAPI, Q-04); Clip Studio + libraries (ADR-0004); clip-battery discrimination and preference testing; tuning×testing fusion (Q-14); live spectrograms/waveforms incl. flat-vs-EQ side-by-side. Exit: import a song, build clips, run a battery, read real statistics.
- **Phase 4 — The Fingerprint Lab.** Measurement engine (sweeps + synchronized capture, reseating statistics); guided fingerprint capture; personal library; headphone-to-headphone matching with re-measure verification (Q-12, TB-14, TB-16). Exit: two of your headphones, statistically matched, verified.
- **Phase 5 — The ecosystem.** Fingerprint export/import + bridging with per-frequency confidence (Q-15, TB-18/19); community features as scoped by Q-10; distribution polish — code signing / SmartScreen, winget, auto-update (explicitly deferred by the user 2026-08-22: "pushed back for now"). Exit: import a reviewer's library, audition a headphone you don't own, see honest confidence bands.

Cross-cutting from Phase 1 onward: everything recorded — full provenance on every test, measurement, and comparison (ADR-0006).
