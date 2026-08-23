# Fletcher

A modern, open-source frontend for Equalizer APO on Windows — parametric EQ that is genuinely pleasant to use, plus the feature no EQ frontend has ever shipped: honest, level-matched, blind ABX listening tests with real statistics. Named for the Fletcher–Munson equal-loudness curves.

## Current status (2026-08-23)

**Phase 3, mid-flight — a working, daily-drivable app.** Shipped (HD650 + Schiit Modi 3+, Equalizer APO 1.4.2 + Peace installed): live curve/strip EQ editing, presets (AutoEQ/Peace import, filter clipboard), global reference loudness, tray + Ctrl+Shift+A level-matched A/B, branching undo graph + Q-24 history inspector, Settings v1, **Clip Studio** (ffmpeg track engine w/ bypass/EQ play methods, Resolve-grade timeline + velocity scrub, clips/moments, spectrogram/FFT scopes + satellite windows, signal generator with mixes/modulators/JSON recipes, yt-dlp link import), and the **Listening Lab finished (ADR-0012)**: unified trial engine — ABX and blind preference protocols, system-audio or clip-battery material (exclusive dual-bus testing mode, per-clip LUFS trims), fixed-N or SPRT-adaptive stopping, any-vs-any contenders incl. history nodes, the finder + batteries, per-tag breakdowns, redesigned room per the LabHome/TrialRoom artboards. First real ABX result: 13/16, p=0.011. Awaiting live verification: the clip-trial audio path, preference + adaptive flows.

## Repo layout

| Path | Contains |
|---|---|
| `crates/fletcher-core/` | Engine lib: lossless APO config parser/writer (`config.rs`), RBJ biquad DSP (`dsp.rs`), APO detection (`apo.rs`), devices + IPolicyConfig default-switch (`devices.rs`), atomic writes w/ Windows retry (`fsx.rs`), preset store (`presets.rs`), .peace importer (`peace.rs`), binomial/xorshift (`stats.rs`). ~30 tests incl. real-fixture round-trips (`tests/`). Spike probe in `examples/`. |
| `app/` | Tauri 2 + React/TS. `src-tauri/src/lib.rs` = the whole command surface + tray/hotkeys/watcher/ABX session engine. `src/App.tsx` = the whole UI (MainApp + PopoutHistory + HistoryTree). `src/App.css` = ADR-0010 tokens. |
| `design/` | The design canvas working files (.dc.html artboards; published at claude.ai/code/artifact/759d400e-2583-45f9-bbd0-3008d41f8cc8). Reference, not runtime. |
| `docs/` | The knowledge system — see map below. |

**Run:** `cd app && npm run tauri dev`. **Verify:** `cargo test --workspace`, `cargo clippy --workspace --all-targets` (CI enforces `-D warnings`), `npx tsc --noEmit` in `app/`. Data dir: `%APPDATA%\Fletcher\` (layout documented in ARCHITECTURE).

## Doc map

| File | Contains | Read when |
|---|---|---|
| `docs/VISION.md` | Why this exists (still DRAFT; content superseded-ish by FEATURES) | Starting any session |
| `docs/ARCHITECTURE.md` | Components, the two A/B paths, on-disk layout | Touching design |
| `docs/FEATURES.md` | The full feature map + product principles (honesty, rooms, no-claims, automation-axis) | Scoping any feature work |
| `docs/decisions/` | ADR-0001..0010 (Rust, Tauri, UX laws, wiring, A/B, visual direction) | Before proposing design changes |
| `docs/OPEN-QUESTIONS.md` | Q-01..Q-24 with statuses; several carry hard-won platform laws (Q-20: satellite webviews get no DOM keys → focus-scoped OS shortcuts) | Before designing anything new |
| `docs/TESTBED.md` | TB-01..TB-24 edge cases | Before AND after design changes |
| `docs/GLOSSARY.md` | Domain vocabulary | A term is unfamiliar |
| `docs/research/` | Verified external facts (APO mechanics + live spike results, AutoEq formats, frontend landscape) | Verifying claims about the outside world |
| `docs/ROADMAP.md` | Phases and current focus | Starting any session |
| `docs/WORKLOG.md` | Session journal | Resuming after a gap |

## Rules for every agent session

1. **Orient first.** This file, `docs/ROADMAP.md`, and the last 2–3 WORKLOG entries.
2. **Never re-litigate an Accepted ADR.** Supersede with a new ADR if truly needed; the user decides.
3. **Every settled decision → ADR; every unresolved debate → OPEN-QUESTIONS (Q-ID); every edge case → TESTBED (TB-ID) the moment it's raised.**
4. **End every session with a WORKLOG entry.**
5. **Living docs describe *now*** — update ARCHITECTURE/GLOSSARY/ROADMAP in place; history lives in ADRs/worklog/git.
6. **Claims about Equalizer APO or Windows behavior go in `docs/research/` with a source or a local experiment.**
7. **Authority:** ADRs and TESTBED are normative; ARCHITECTURE/README derived.
8. **Prime directive:** never damage the user's audio setup — never touch config lines other tools own (ADR-0007), never surprise-full-volume (TB-20), atomic writes only (TB-11).
9. **Working style that works here:** ship small, let the user drive taste by reacting to the real app (dev server hot-reloads), file every idea the moment it's spoken, and fix root causes (the user will keep pushing — correctly — until the foundation is right).
10. **Every feature ships to the satellites too (user directive 2026-08-23).** Any tree/inspector/window feature must be traced end-to-end through the pop-out path before handoff: props actually passed in `PopoutHistory`/`PopoutDiff`, a `hist-cmd` payload + main-listener case, capabilities coverage (Q-20 law #2), and popout-only interference (focus-grabbing, no-DOM-keys). Satellites can't be driven by automation — explicitly ask the user to exercise the pop-out when testing, and never declare a feature done on main-window behavior alone.
