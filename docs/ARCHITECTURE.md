# Architecture

> Current-state design. Very early — most of this is a sketch pending the idea-mapping sessions and Q-01/Q-04.

## The one structural insight

Equalizer APO watches its config files and hot-reloads on change, glitch-free. Fletcher therefore has **two fundamentally different A/B paths**:

### Path 1 — system-wide (config swap)
For EQing whatever the system is playing. Fletcher atomically rewrites (or re-points an `Include:` line in) the APO config; APO reloads instantly. A global hotkey flips between candidate configs. Blind mode randomizes the assignment. Cheap, glitch-free, works with any source app.

### Path 2 — track mode (in-app engine) — *built 2026-08-23, ADR-0011*
The engine (fletcher-core `playback.rs`/`signal.rs`/`dsp.rs` realtime layer + app `engine.rs`):

- Decode: **ffmpeg** (managed tool, fetch-on-demand) → per-(track, rate) f32 PCM cache; flat LUFS measured once and stored (ADR-0011 superseded the symphonia sketch).
- DSP: the same RBJ biquads as the curve renderer (`BiquadState`/`ChainProcessor`, cross-validated against `magnitude_db`).
- Three play methods (ADR-0011): **bypass** (curation default — exclusive, no EQ, level-matched toward the reference), **through your EQ** (opt-in regular player — shared path, APO applies, normal A/B), **testing** (Lab phase ③ — dual buses always processed through identical buffering, ~15 ms equal-power crossfade, true-LUFS trim via `ebur128`; TB-12-clean).
- Output: WASAPI exclusive (format ladder + aligned-period retry from the spike) or shared; every stream ramps in and fades out (TB-20, engine-enforced); sessions serialized so streams never overlap.

## Components (sketch)

| Component | Job | Notes |
|---|---|---|
| Config engine | Parse/generate APO config grammar, round-trip safely, atomic writes | Coverage scope: Q-09. Must coexist with Peace-owned configs (TB-02). |
| Device layer | Enumerate audio devices (MMDevice), device arrival/removal + default-change events (profile auto-switch), match APO's device-selection semantics, find APO install via registry | TB-05 |
| Preset store | Presets + journaled edit history as a branching, jumpable tree; any two nodes blind-testable | Q-17, ADR-0006 provenance |
| Filter math | Biquad coefficient + frequency-response computation | Shared by curve renderer, track engine, preamp auto-gain |
| Track engine | Decode → DSP → crossfade → WASAPI out | Path 2 above |
| ABX core | Trial sequencing, randomization, level matching, statistics, session history | Design: Q-05, Q-06 |
| Clip library | Data model + Clip Studio curation view (waveform/spectrogram, spans, tags, genres) | ADR-0004; consumed by ABX core and tuning loop |
| Measurement engine | Synchronized sweep playback + mic capture, impulse-response extraction, reseating statistics, fingerprint store, bridging | Fingerprint Lab (Q-12, Q-15); later phase but influences audio-engine design |
| UI shell | Tauri 2 + TypeScript/React; draws only — receives compact Rust-computed binary frames over Tauri channels for live visuals | ADR-0005 |
| Tray/hotkeys | Background presence, global A/B toggle | |

## On-disk layout (v1, settled 2026-08-23)

Everything under `%APPDATA%\Fletcher\`. Design rules: **human-readable and
portable wherever feasible** (presets are plain APO syntax; records are
pretty JSON), **export = the file itself** (sharing a preset, a history
tree, or later a fingerprint means handing someone the file), and each
subsystem owns one folder.

| Path | Contents | Format |
|---|---|---|
| `state.json` | active preset, A/B side, reference level | JSON |
| `presets/<name>.txt` | EQ chains | APO config syntax (portable, Peace/AutoEQ-compatible) |
| `history/<preset>.json` | undo graphs, one tree per preset (survive restarts; export/import = this file) | JSON `{version, current, nodes[]}` with full snapshots |
| `sessions/<id>.json` | ABX session records — labeled trial logs, provenance | JSON |
| `autoeq/` | cached AutoEQ index + fetched preset files | md / APO syntax |
| `tools/` | managed external tools (ffmpeg, ffprobe, yt-dlp), fetch-on-demand | exes (ADR-0011) |
| `cache/pcm/` | decoded track PCM, per (track, rate); purgeable | raw f32le |
| `clips/library.db` | the clip library: tracks (+ flat LUFS), clips/moments/tags/markers/batteries as they land | SQLite — the deliberate exception to the human-readable rule (ADR-0011) |
| `fingerprints/` *(Phase 4)* | measured fingerprint library; the interchange files (Q-15) are these, exported | TBD by Q-15 (metadata-rich) |
| `calibration/` *(Phase 4)* | mic calibration curves | freq/dB text |

Media itself (imported tracks) is referenced in place, never copied, unless
the user asks; annotations and derived data live in Fletcher's folders.

## Implementation state (2026-08-23)

**Command surface** (`app/src-tauri/src/lib.rs`): `eq_state` (live config → filters + computed responses, per-source ownership), `set_fletcher_chain` (chain → matched preamp → atomic write; saves into active preset; lands on A), preset suite (`presets_state/switch/create/copy_from_source/duplicate/delete/rename`, `preset_create_from_chain` — node promote, auto-suffixed, non-activating), `autoeq_search/import`, `devices_list/device_set_default` (IPolicyConfig), `ab_info/ab_set`, ABX suite (`abx_start/audition/vote/reveal/cancel/sessions` — X assignments never leave Rust; `abx_start` optionally takes two arbitrary chains + names for node-vs-node, else classic active-vs-flat; results carry both chains + `referenceDb` as provenance), `parse_filters` (clipboard paste), inspector engine access (`chain_curves` — batched responses + matched preamps for arbitrary chains on the shared 200-pt grid; `preview_chain` — level-matched fletcher.txt-only audition, no preset write, no A/B reset, refused mid-ABX), `history_save/load/export/import`. Level law: `matched_preamp` normalizes every chain's log-grid mean response to the global reference (`referenceDb`, default −8), tightened to clip-safe (TB-06).

**Event channels**: `apo-config-changed` (Rust watcher → UI refresh, suppressed mid-drag), `ab-changed` (hotkey/tray flips), `abx-audition` (hotkey cycling in sessions), `hist-sync` (main → pop-out tree, snapshots included since Q-24), `hist-cmd` (pop-out/Rust shortcuts → main: jump/del/undo/redo/edit/preview/restore/abx/promote/compare/popdiff; Rust emits `restore` when the history pop-out is destroyed so previews never outlive it), `diff-sync`/`diff-hello` (main → difference pop-out: the resolved compare — node/base/series snaps — re-emitted on every tree change; the satellite computes its own curves via `chain_curves`).

**Frontend anatomy** (`app/src/App.tsx`): `App` routes by `?view=history` → `PopoutHistory` else `MainApp`. `MainApp` holds all state: eq state, presets, A/B, ABX, selection (primary + multi-set), the undo graph (`hist` ref: nodes with snapshots + optional note/pin; the rail), tooltips (1.5s still-hover layer), y-scale (interaction-inert). `HistoryTree` is the shared canvas + inspector component (Q-24): right-side panel with rename/note/pin, parametric diff (greedy kind+log-Fc pairing), level-matched difference curve (`DiffPlot`), any-N-node compare vs a picked base, listen previews, node-vs-node blind test, promote/copy; edge thickness on the canvas = mean |audible diff| per edge. In the pop-out, mutations and audio route to main over `hist-cmd` (main owns truth); stateless curve reads invoke Rust directly. Editing writes are optimistic + 60ms-throttled; mid-drag server echoes merge under local drag values.

**Platform laws** (verified the hard way): satellite webviews get no DOM keyboard events → focus-scoped OS shortcuts (Q-20); Windows renames need sharing-violation retries (`fsx`); Peace zeroes peace.txt when toggled off (its state lives in `.peace` files, importable); the main window close-hides to tray, satellites really close.

## Committed so far

- Rust core (ADR-0001).
- Equalizer APO as the system-wide DSP engine — Fletcher is a frontend, not a driver.
