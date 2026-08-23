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

## Committed so far

- Rust core (ADR-0001).
- Equalizer APO as the system-wide DSP engine — Fletcher is a frontend, not a driver.
