# Worklog

Append-only session journal. Newest entry first.

**Entry template**
```
## YYYY-MM-DD — short title
- **Did:** what changed in the repo/design
- **Learned:** surprises, verified facts, dead ends
- **Next:** the single most useful next step
```

---

## 2026-08-22 — Design session: five iterations to a settled EQ-tab direction

- **Did:** Built a design canvas (claude.ai/code/artifact/759d400e-…) with three directions from real HD650 data; user killed Swiss, chose Console, then drove five iterations: v3 curve/editor split, v4 transposed horizontal strip + tooltip-not-panel + bidirectional graph↔strip editing, v5 arc-fill gain gauges (needles rejected as unglanceable) and type-as-dropdown (name · curve glyph · caret). Persistent A/B bar replaced the floating Blind-test button. Settings artboard approved early and untouched since. Vocabulary audit: invented labels (PRINTS, BENCH, response-curve toggle) purged after user flagged them as unmappable to our docs. New ideas filed: pop-out windows with synced state (Q-20), post-"Load a track" flow (Q-21).
- **Learned:** The user's design taste is concrete and fast when given real candidates to react against; "show 3, iterate on reactions" works. Redundant views (rail duplicating graph) read as mess; amount-of-ink beats needle-angle for glanceability; every UI word must trace to FEATURES.md or it reads as hallucination.
- **Next:** ADR-0010 (visual direction); port v5 into the app; design Listening Lab + Clip Studio artboards (resolves Q-21).

## 2026-08-22 — Phase 1 opens: workspace, Tauri scaffold, config engine v1

- **Did:** Restructured to a cargo workspace: `crates/fletcher-core` (engine lib; spike moved to examples) + `app/` (Tauri 2 + React/TS via create-tauri-app, product name Fletcher, identifier net.asal.fletcher). Built the config engine's first component: a lossless line-preserving APO config parser (`config.rs`) — raw bytes kept per line, parsing as a view, byte-for-byte round-trip guaranteed; parses Preamp/Filter (12 kinds)/Include/Device/Channel, everything else degrades to Unknown. 7 integration tests green against real fixtures copied from the live APO install (Peace HD650 preset, root config, REW exports, multichannel). CI updated for workspace + frontend build. Mid-session feature idea filed: moment isolation + spectral similarity + future MCP agent hook (Q-18).
- **Learned:** REW exports (which APO reads and users Include) use European locale numbers — decimal commas and dot thousands-separators (`8.000` = 8000 Hz). Comma lines safely degrade to Unknown; the dot-thousands ambiguity needs REW-dialect detection before Fletcher ever interprets such files → TB-22.
- **Next:** Tauri command surface: read APO config via fletcher-core and render it in the UI (first end-to-end slice), then device layer + fletcher.txt writer + hotkey A/B.

## 2026-08-22 — Development kickoff: ADR-0007/0008, license, both spikes done

- **Did:** Kickoff decisions via user Q&A: include-line wiring (ADR-0007), AutoEQ fetch-on-demand (ADR-0008), spikes-first, CI-from-start; Apache 2.0 license added. Live spike results on the dev machine (APO 1.4.2 + Peace, HD650, Schiit Modi 3+): config dir writable without elevation (Users: FullControl); Fletcher wired in as `Include: fletcher.txt` alongside Peace's line; hot-reload through included files confirmed audibly — "basically 0 latency" at 1 s toggle cadence; WASAPI exclusive mode confirmed to bypass APO (Q-04) via first Rust code (`src/bin/spike_bypass.rs`, wasapi 0.24) — but it evicts other apps ungracefully (Apple Music "error playback", no auto-recover) and bypasses Windows volume (own gain stage mandatory, TB-20). GitHub Actions CI added (fmt/clippy/test, windows-latest).
- **Learned:** Peace zeroes peace.txt when toggled off (TB-09 churn is real). Exclusive-mode format negotiation on Modi 3+: 24-bit int @ 48 kHz.
- **Next:** Confirm ADR-0009 (exclusive-mode bypass as primary), then scaffold Phase 1 (Tauri app + config engine with round-trip tests).

## 2026-08-22 — Gap pass: profiles, loudness mode, history tree, safety

- **Did:** Seven-gap review with the user, all filed: per-device profiles with device-level auto-switch (headphone-level is a manual quick-switcher — analog jacks are invisible to Windows); loudness-compensation mode, off by default, anchored-reference design (Q-16, TB-21); preset history as a branching jumpable tree with blind A/B between any two nodes (Q-17); per-channel EQ / L-R balance from day one; mic calibration import (UMIK-2 / iMM-6 / OmniMic formats — user's real cal files as fixtures); sweep ear-safety (TB-20); code signing deferred to Phase 5 by user. Scope note: headphones first, room correction deferred.
- **Learned:** User owns a USB UMIK-2 — real hardware for Phase 4 measurement-engine development. Volume anchoring (RME-style reference point) makes loudness comp workable without a mic; USB-HID volume sync makes many DAC knobs visible.
- **Next:** Phase 0 exit: Q-04 spike, normative VISION rewrite, fingerprint-format sketch; then Phase 1 scaffold.

## 2026-08-22 — Idea-mapping session: the vision lands, six ADRs

- **Did:** Full feature brain-dump captured into FEATURES.md. Decisions locked: ADR-0002 (grey-don't-hide UI), ADR-0003 (flat reference + level matching by default), ADR-0004 (user-curated clip libraries / Clip Studio), ADR-0005 (Tauri 2 + TS/React shell), ADR-0006 (scope by sequencing, not cutting; everything-recorded provenance). Fingerprint Lab vision clarified: fingerprints are coupled HpTF∘ear measurements; personal libraries; export/import; bridging via shared headphones for at-home virtual auditioning (valid mainly <5–6 kHz). Q-01/02/06/07/11/13 resolved; Q-15 (fingerprint interchange format) opened; TB-13…TB-19 added. ROADMAP rewritten as six everything-ships phases. APO auto-install: no, for now.
- **Learned:** The reviewer-ecosystem/bridging idea makes the fingerprint *format* strategic — design it early (Phase 0/1), ship the Lab late. Preference vs. discrimination tests need separate statistical treatment (TB-15). Clip libraries being per-track + genre-tagged gives the stats engine content-type dimensions for free.
- **Next:** Q-04 research spike (how track-mode playback bypasses APO) + verify APO hot-reload semantics locally; then rewrite VISION as normative and scaffold Phase 1.

## 2026-08-22 — Founding session: name, stack, doc system

- **Did:** Project named **Fletcher** (after Fletcher–Munson). Rust core committed (ADR-0001). Documentation system bootstrapped mirroring the Humanity-Tech-Tree standard: CLAUDE.md rules, ADRs, OPEN-QUESTIONS (Q-01…Q-10), TESTBED (TB-01…TB-12), GLOSSARY, ROADMAP, research survey of the frontend landscape. Git remote added (github.com/andrewsalinas09/Fletcher).
- **Learned:** The modern-frontend niche is open — AQUA (Electron/React) was the only serious attempt and is abandoned. APO's config hot-reload makes system-wide A/B nearly free, but track-mode A/B **cannot** ride it (double-processing trap, TB-03) and needs an in-app engine. Level matching (LUFS) is what makes ABX statistics honest — and it's the feature no incumbent has.
- **Next:** Idea-mapping conversation with the user — capture the full feature vision, then rewrite VISION.md and start resolving Q-01/Q-02/Q-07.
