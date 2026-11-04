# ADR-0021 · Targets, measurements and preference bounds

- **Status:** Accepted (2026-11-04). Resolves Q-31 (issue #13). Supersedes the "measurement overlays" sketch in FEATURES.
- **Research:** docs/research/targets-and-preference-bounds.md.

## Context

A user asked for squig.link-style overlays: target curves and preference bounds on the graph, selectable like presets. A target is a target *response*, so it only means something next to the headphone's own measured response. Fletcher's graph shows the correction (the EQ), so the feature needs the headphone's response first. Only AutoEq's data is licensed (MIT); squig.link's, PEQdB's and the Headphones.com preference bounds are not.

## Decision

**The rule that places everything (owner):** information lives with what owns it, and views only show it. Rarely used things stay out of the way because they're rare, and are simple and obvious when needed. A feature's controls stay together, as one section to polish as a unit.

### What exists, and where it lives

- **A measurement** is a headphone's measured response (L/R where available), with its source and the **rig** it was measured on (the fake head and ear). From AutoEq's raw measurements (searched like presets, fetched on demand and cached, ADR-0008, credited to the source) or imported from the user's own file (REW text or AutoEq CSV). Separate from presets.
- **The output → headphone binding** belongs to the output device: a "Headphone" section in its settings modal, beside "open APO's device selector" (search AutoEq, import a file, none). Once bound, it's listed in Settings, the way a device hotkey is. Nothing on the EQ page shows the binding.
- **A target** belongs to no device. A TARGETS section in Settings holds the library: a few featured AutoEq targets, **every** AutoEq target searchable ("it's just data"), the user's own, and imported files. "New target" opens the usual settings modal (poppable, Q-20 rules): a base response plus four knobs (bass: low shelf 105 Hz; ear gain: peak 2.75 kHz; treble: high shelf 2.5 kHz; tilt, dB/octave) with a live preview, or import a file, or "save what I hear now". Choosing a target shows the rig it was made on and that rig's own ear response (its HRTF: AutoEq's diffuse-field files), with a link about the rig. There is no mismatch warning; the rig is shown where you choose.
- **Preference bounds** are a pair of offset curves around a base target. The mechanism is built with the target; built-in bounds come later (below). Imported bounds files are allowed.

### How it's shown

- **The EQ page's ⚙ gets one ghosts section:** other ear (today's "Enable ghost", renamed for what it shows), measurement, predicted (measurement + your EQ), target (with the target chooser under it), bounds, and smoothing. Each is a show/hide toggle.
- **One graph, one axis.** Measurement, predicted, target and bounds are drawn on the EQ graph's dB axis, lined up at 0 dB. No separate response view.
- **Alignment** (research): the measurement is anchored on its mean over the third-octave band at 500 Hz (IEC 60268-7; how Harman presents targets). The target is moved, never the measurement, by the least-squares offset over 100 Hz–10 kHz (AutoEq's published method; the ear simulator's specified range). The predicted response is never refitted: it is the measurement plus the EQ curve exactly as the graph draws it. That curve leaves out the preamp, as Fletcher's graph always has; the preamp is a level, which level matching handles, and drawn in it would push the predicted curve below a target it matches in shape (revised 2026-11-04 while building the UI). Left and right share one offset (from their average), so a real imbalance stays visible.
- **Smoothing:** 1/12 octave by default; none, 1/24, 1/6, 1/3 and ERB offered. The treble is shaded as a gradient, because reliability fades rather than stops (researched 2026-11-04 at the owner's question): it fades in from 6 kHz (7 kHz for in-ears, whose canal resonance moves with insertion depth), where peaks and dips start to move with fit and ears, to full at 10 kHz, where the curve mostly reflects the rig.
- **Labels:** every curve names its source and rig. A measurement says it's one rig's average, not your ears (in device settings). The graph's legend just names the target: "a population's preference" was dropped as clutter (owner, 2026-11-04).

### Acting on it

- **"From target…"** in the preset menu, beside "From REW file…": fits the current output's headphone to the chosen target and creates a new, editable preset (an AutoEq-style fit: AutoEq's smoothing defaults, treble handled the same way). Per-ear when the measurement has L/R and per-ear editing is on.
- **The Lab:** "EQ'd to target A vs EQ'd to target B", ABX and blind preference. The contender picker gets a TARGETS group; each fits the current headphone on the fly and is level-matched like any contender.

## Later (decided, not yet built)

- Fletcher's own built-in bounds (e.g. from Harman's listener segments, cited). The derivation is still to be designed.
- **Measured personal bounds:** a short series of blind comparisons of bass, ear gain and treble around a target, mapping the user's own range of preference ("100% we should do").
- Comparing to another headphone ("make my HD 650 sound like an HD 800 S"): a measurement used as the target.
- Sharing targets and bounds: the file itself is the export.

## Build plan

1. **Core** (`fletcher-core::response`): a response curve on AutoEq's grid (20 Hz–20 kHz, ×1.01); parsers for AutoEq CSV and REW text; interpolation; fractional-octave and ERB smoothing; the 500 Hz anchor and the 100 Hz–10 kHz least-squares fit (median option); target = base + four knobs; predicted = measurement + EQ response (from `dsp`). Unit-tested against AutoEq's own outputs.
2. **Sources:** AutoEq measurement indexes (`name_index.tsv` per source; rig from the path) and targets, searched, fetched and cached; user imports into `%APPDATA%\Fletcher\measurements\` and `targets\` (the file itself plus a small JSON of name, source, rig, form, credit).
3. **Device binding:** the Headphone section in device settings; `deviceSettings.headphone` in state.json; the Settings list once bound.
4. **Settings › TARGETS:** library with featured and search-all; import; the new-target modal (poppable) with the rig and its ear response.
5. **EQ ⚙ ghosts section:** the renamed other-ear ghost, measurement, predicted, target and chooser, bounds, smoothing; drawn on the graph, with labels and the shaded treble; per-ear.
6. **"From target…":** the fit, as a new preset.
7. **Lab:** targets as contenders.

Each step ships on its own and is usable.

**Progress:** steps 1–2 and the backend of 3–5 done 2026-11-04 (`fletcher_core::response`, `headphones.rs`: commands `headphone_search`, `device_headphone_autoeq/_import/_clear`, `headphone_bindings`, `targets`, `targets_search`, `target_preview`, `target_save/_import/_delete`, `eq_overlay`). Found while building: AutoEq's tilt pivots at 632 Hz (`log_tilt`), its bass boost is the RBJ shelf with fc as midpoint (APO's LSC), and its web-app targets are stored without bass, the boost added on top. Next: the UI for 3–5, then the fit (6).

**UI, first pass (2026-11-04):** `src/targets.tsx` holds the feature's components (HeadphonePicker, TargetPicker, TargetEditor, GhostsSection), placed by App.tsx: the Headphone section in device settings; Settings › HEADPHONES (once any output has one; editable while unplugged) and › TARGETS (yours, new, import with optional bounds); the EQ ⚙'s GHOSTS section (other ear, measurement, measurement + EQ, target with its chooser, bounds, smoothing) and a small legend naming each ghost's source. Measurement + EQ is drawn from the live curve, so it follows a drag. Not yet: popping the target editor out into its own window. Steps 3, 4 and 5 touch pop-outs (the target modal), so rule 10 applies.

## Consequences

- AutoEq's file layout becomes a second runtime interface (as ADR-0008 already accepted for presets); its measurement folders are versioned by source and rig.
- No unlicensed data ships. squig.link, PEQdB and Headphones.com data reach Fletcher only as files a user imports.
