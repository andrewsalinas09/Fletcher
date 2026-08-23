# Research: the Equalizer APO frontend landscape

*Surveyed 2026-08-22. Conclusion: the "modern frontend" niche is genuinely open.*

## The engine: Equalizer APO

- Windows APO (Audio Processing Object) — system-wide DSP installed per-device. [SourceForge](https://sourceforge.net/projects/equalizerapo/), mirrored at [github.com/mirror/equalizerapo](https://github.com/mirror/equalizerapo).
- **The entire control surface is text config files** under `<install>\config\` (root `config.txt`, plus `Include:`-ed files). Install path discoverable via registry.
- **Hot reload:** APO watches the config files and re-applies on change, glitch-free. This is the property Fletcher's instant-everything UX and system-wide A/B ride on. *(Verify precise reload semantics — debounce, partial-write behavior — with a local experiment before relying on details; atomic rename-into-place is the safe write strategy regardless.)*
- Grammar includes: `Preamp`, `Filter` (RBJ biquad types: PK/LS/HS/LP/HP/NO/AP/BP + variants), `GraphicEQ`, `Convolution` (impulse WAV), `Include`, `Device`/`Channel` selectors, `Copy` (channel matrix), `Delay`, expression evaluation, `If`/`Eval`.
- Known blind spot: sources using ASIO or WASAPI-exclusive bypass APO entirely (TB-04).

## Frontends

| Project | Stack | State | Notes |
|---|---|---|---|
| [Peace](https://sourceforge.net/projects/peace-equalizer-apo-extension/) | Closed-ish, dated toolkit | Active, de facto standard | Feature-rich, UI widely disliked; no blind testing. Its configs are what Fletcher must coexist with (TB-02). |
| [AQUA](https://github.com/h39s/AQUA) | Electron + React + TS (+ C++ glue) | **Abandoned** ("no longer under development") | The one prior modern attempt. AutoEQ integration, auto-preamp, response graph. Caps: 20 filters, single device, no A/B/ABX. Worth mining for config-writing and AutoEQ plumbing (open source). |
| [equalizer-apo-editor](https://github.com/Eustakius/equalizer-apo-editor) | — | Niche | Stock-Editor.exe replacement; claims full config grammar + live response graph. Reference for grammar coverage (Q-09). |
| [DynamiQ](https://github.com/Brad331/DynamiQ) | — | Niche | Dynamic EQ via custom text files. |
| [EqualizerAPO-Frontend](https://github.com/TulioAdriano/EqualizerAPO-Frontend) | — | Tiny | Band-gain slider frontend. |

## Adjacent resources

- **[AutoEQ](https://github.com/jaakkopasanen/AutoEq)** — measured EQ presets for thousands of headphones, multiple targets (Harman, oratory1990-derived); emits APO-format parametric configs. Delivery question: Q-08.
- **RBJ Audio EQ Cookbook** — the biquad coefficient formulas APO uses; Fletcher's shared filter-math module implements these.
- **EBU R128 / ITU-R BS.1770** — loudness (LUFS) standard behind level matching; Rust crate `ebur128`.
