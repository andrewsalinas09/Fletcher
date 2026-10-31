# ADR-0018: A measured setup check and a diagnostics zip

- **Status:** Accepted (built 2026-10-31, unreleased)
- **Date:** 2026-10-31
- **Source:** A Reddit tester. Fletcher's import and UI worked, "but it's not changing the audio output (sound the same with EQ enable or disabled) while PEACE does … My APO installation is using LFX/GFX." They cut everything above 1 kHz by 20 dB and heard no change. Owner: "the next thing we need is the button to test, live feedback checking like a check setup … and then in settings an export diagnostic which exports a zip folder of all that data including the filter, what other stuff on their computer enabled."

## Context
Fletcher writes fletcher.txt and makes sure config.txt includes it. Whether Equalizer APO then plays it depends on things Fletcher didn't write:
- APO installed on the output, in the stage it runs (pre-mix and/or post-mix);
- Windows enhancements switched on for the device;
- no `Device:` line above Fletcher's Include that excludes the output;
- no routing afterwards that replaces what Fletcher did.

The "my EQ does nothing" reports so far have been undetectable from inside the app: the curve looks right and the file is right. Some were real (TB-32, APO removed by a Windows update). Some were people who couldn't hear a subtle difference under level matching.

Measured (docs/research/apo-scope-and-loopback.md): WASAPI loopback records APO's output exactly. A `Device:` line that doesn't match, placed in config.txt above the include, silences Fletcher completely. Selections made inside an included file end with that file.

## Decision
Settings gets a **SETUP** section.
1. **Check setup** runs two parts:
   - **Static checks** (`setup::setup_checks`):
     - APO and its version;
     - APO on the default output, and Windows enhancements on;
     - which APO stages the device has (`apo::device_apo_stages`): pre-mix-only fails;
     - config.txt includes fletcher.txt;
     - the last `Device:` line above that Include matches the output, matched like APO matches: every word found in "Device_name Connection_name GUID", `;` alternatives, `all`;
     - any `Stage:` without post-mix, and `Copy:` routing.
   - **A live test** (`setup::path_test`):
     - plays a −50 dBFS 997 Hz tone on the default output;
     - appends `Preamp: -20 dB` to fletcher.txt and measures the tone on loopback with a Goertzel filter, so music already playing barely matters;
     - if the output drops by 15 dB or more, the path works (post-mix);
     - if not, it retries with `Stage: pre-mix` and reports "APO only runs before mixing here" or "doesn't reach your output".
   - While the test runs, it holds the writer lock (`LAST_WRITTEN_LEVEL`), so no edit, fade or flip interleaves. Every test write is the current file plus a cut, never louder. A drop guard restores the file byte-for-byte, even on error. It refuses during a blind test or a hearing session.
2. **Export diagnostics…** saves a zip, and nothing is uploaded. It contains:
   - `report.txt`: versions, the default output, APO's match text for it, its stage classes, every check, the last live result, and installed programs and running processes whose names suggest audio (name match only);
   - `registry/`: everything under `HKLM\SOFTWARE\EqualizerAPO`, and every playback endpoint with its state, the enhancements switch, APO status, and every effect CLSID with its registered name, so Dolby, Nahimic and Realtek effects show by name;
   - `apo-config/`: APO's config folder, minus programs, icons and anything over 2 MB;
   - `fletcher/`: state.json, the presets, and `eq_state.json` (what Fletcher believes is playing; the report summarizes it: active preset, A/B side, counted sources);
   - the default output's mix format (rate, channels, mask) in the report, and every value of its registry Properties in `registry/default-output-properties.txt`;
   - `outside-includes/`: files config files include from outside APO's folder, followed through nested includes.
   Exporting without a prior check runs the live test first; its result is the most useful line in the report.

## Why
- **Measure rather than infer:** a wrong path looks identical to a right one from the file side, and the static checks can only catch causes someone thought of. Loopback measurement catches the rest, including driver quirks.
- **A −20 dB cut, not a muffling low-pass you listen for:** it needs no listening, can't be mistaken for "subtle", and only ever lowers the level (TB-20).
- **No automatic fix for a `Device:` scope in config.txt:** a line placed before the Include would also re-scope every line after it, which other tools own (ADR-0007). The check names the line and says what to change.
- **Programs by name match only:** a full list of installed software is mostly noise and more personal than a bug report needs. The effect CLSIDs on each device are the stronger signal anyway.

## Consequences
- A Stage option for outputs where APO runs only pre-mix (writing `Stage: pre-mix` at the top of fletcher.txt) waits for a confirmed case (Q-30).
- `eq_state` still counts Fletcher as active when config.txt scopes it away. The header's "Peace + Fletcher" can therefore be wrong until it reads the same scope (Q-30).
