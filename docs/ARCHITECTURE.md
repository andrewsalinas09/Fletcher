# Architecture

> Current-state design. Very early — most of this is a sketch pending the idea-mapping sessions and Q-01/Q-04.

## The one structural insight

Equalizer APO watches its config files and hot-reloads on change, glitch-free. Fletcher therefore has **two fundamentally different A/B paths**:

### Path 1 — system-wide (config swap)
For EQing whatever the system is playing. Fletcher atomically rewrites (or re-points an `Include:` line in) the APO config; APO reloads instantly. A global hotkey flips between candidate configs. Blind mode randomizes the assignment. Cheap, glitch-free, works with any source app.

### Path 2 — track mode (in-app engine)
For "import a song and flip between EQ'd/bypass." **Trap:** anything Fletcher plays through the normal device is *also* processed by APO — toggling the config is not sample-accurate and double-processes. So track mode uses an in-app engine:

- Decode: `symphonia` (FLAC/MP3/AAC/WAV/OGG).
- DSP: same RBJ-cookbook biquads APO uses, implemented in Rust (needed anyway to draw the response curve).
- Switch: shared playhead, short equal-power crossfade (~5–20 ms) between processed and bypass buses — seamless, position never jumps.
- Output: WASAPI, with APO taken out of the loop for the session (mechanism TBD — Q-04: exclusive mode vs. temporarily blanking the device config vs. other).
- Level matching: `ebur128` computes LUFS of both paths over the loop region; compensating gain applied before the test; matched levels shown to the user.

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
| `clips/` *(Phase 3)* | clip libraries: per-track folders for media references + annotations; queryable records in the database | folders + SQLite |
| `fingerprints/` *(Phase 4)* | measured fingerprint library; the interchange files (Q-15) are these, exported | TBD by Q-15 (metadata-rich) |
| `calibration/` *(Phase 4)* | mic calibration curves | freq/dB text |

Media itself (imported tracks) is referenced in place, never copied, unless
the user asks; annotations and derived data live in Fletcher's folders.

## Implementation state (2026-08-23)

**Command surface** (`app/src-tauri/src/lib.rs`): `eq_state` (live config → filters + computed responses, per-source ownership), `set_fletcher_chain` (chain → matched preamp → atomic write; saves into active preset; lands on A), preset suite (`presets_state/switch/create/copy_from_source/duplicate/delete/rename`), `autoeq_search/import`, `devices_list/device_set_default` (IPolicyConfig), `ab_info/ab_set`, ABX suite (`abx_start/audition/vote/reveal/cancel/sessions` — X assignments never leave Rust), `parse_filters` (clipboard paste), `history_save/load/export/import`. Level law: `matched_preamp` normalizes every chain's log-grid mean response to the global reference (`referenceDb`, default −8), tightened to clip-safe (TB-06).

**Event channels**: `apo-config-changed` (Rust watcher → UI refresh, suppressed mid-drag), `ab-changed` (hotkey/tray flips), `abx-audition` (hotkey cycling in sessions), `hist-sync` (main → pop-out tree), `hist-cmd` (pop-out/Rust shortcuts → main: jump/del/undo/redo).

**Frontend anatomy** (`app/src/App.tsx`): `App` routes by `?view=history` → `PopoutHistory` else `MainApp`. `MainApp` holds all state: eq state, presets, A/B, ABX, selection (primary + multi-set), the undo graph (`hist` ref: nodes with snapshots; the rail), tooltips (1.5s still-hover layer), y-scale (interaction-inert). `HistoryTree` is the shared canvas component. Editing writes are optimistic + 60ms-throttled; mid-drag server echoes merge under local drag values.

**Platform laws** (verified the hard way): satellite webviews get no DOM keyboard events → focus-scoped OS shortcuts (Q-20); Windows renames need sharing-violation retries (`fsx`); Peace zeroes peace.txt when toggled off (its state lives in `.peace` files, importable); the main window close-hides to tray, satellites really close.

## Committed so far

- Rust core (ADR-0001).
- Equalizer APO as the system-wide DSP engine — Fletcher is a frontend, not a driver.
