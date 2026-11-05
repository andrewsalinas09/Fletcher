# Importing into Fletcher

Everything Fletcher can read from a file or from AutoEQ, where you find it, which files it accepts, what it does with the numbers, and where it keeps them. Files you import are copied unchanged into Fletcher's folder (`%APPDATA%\Fletcher\`), so moving or deleting the original later breaks nothing.

| You have | It becomes | Where |
|---|---|---|
| A headphone's measured response | The headphone on an output | Output settings › Headphone |
| A target curve | One of your targets | Settings › TARGETS › import… |
| Preference bounds (two files) | The band around one of your targets | The target's editor › Bounds › add… |
| A REW filter export | A preset | Preset menu › New preset… › REW file… |
| Filter lines (copied text) | Filters in the current preset | Ctrl+V on the EQ page |
| Another tool's EQ (Peace, …) | A preset | Preset menu › New preset… › *tool*'s chain |
| An AutoEQ correction | A preset | Preset menu › type the headphone's name |
| Your headphone and a target | A preset | Preset menu › New preset… › A target… |
| A history file | A preset's undo tree | History panel › import… |

## Response files (measurements, targets, bounds)

Measurements, targets and bounds are all the same kind of file: a frequency response, one frequency and one level (dB) per line. Fletcher reads the two formats nearly everyone exports.

**REW text** (Room EQ Wizard › File › Export › Export measurement as text; also what squig.link databases and most measurement rigs publish):

```
* Measurement data measured by REW V5.31
* Freq(Hz)	SPL(dB)	Phase(degrees)
20.000	-6.910	0.0
20.291	-6.838	0.0
```

**AutoEQ CSV** (AutoEQ's measurements and targets):

```
frequency,raw
20.00,-6.91
20.20,-6.87
```

The rules, exactly as the reader applies them:

- **Comments:** lines starting with `*`, `#` or `//`, and empty lines, are skipped.
- **Columns:** frequency first. The level is the second column, unless a header row names a column `raw` (AutoEQ's result files have many columns: `frequency,raw,smoothed,error,…`), in which case that one. Further columns, such as phase, are ignored.
- **Separators:** tabs, spaces or semicolons; or commas when the line has none of those. In a tab, space or semicolon separated line, commas are decimal points (`20,5` is 20.5), as in REW exports from European locales.
- **Header rows:** any line whose first field isn't a number is a header, and only used to find `raw`.
- **What's kept:** positive, finite frequencies with finite levels, sorted by frequency; a repeated frequency keeps its first value. At least two points.
- **Any grid:** points can be at any frequencies and spacing. Between points Fletcher interpolates linearly in log-frequency. Outside the file's range it draws nothing and invents nothing: a measurement that starts at 20 Hz has no line below 20 Hz.
- **Levels:** absolute levels are fine (a measurement at 94 dB SPL reads the same as one at 0 dB), because everything is lined up by shape (see *How the curves line up*).
- **Extensions:** `.txt`, `.csv` and `.frd` in the file dialogs; the contents decide the format, not the extension.

A file that doesn't fit says why ("no frequency response found: expected lines of frequency and dB"), with the file's name.

## Headphone measurements

An output's **Headphone** (its settings, from the output menu's ⚙ or Settings › HEADPHONES) is what the EQ graph's Measurement and "Measurement with your EQ" draw, and what **A target…** fits.

**From AutoEQ:** search by name. Results list who measured it and on which rig (oratory1990 on a GRAS 43AG, Rtings on a HMS II.3, …), because the same headphone measures differently on different rigs, and a target is made for one rig. Choosing one fetches that measurement from AutoEQ's repository on GitHub and caches it. AutoEQ's measurements are one curve for both ears.

**Your own:** *import a measurement…* takes one or two response files.

- **One file:** used for both ears.
- **Two files:** one per ear. The file dialog hands them back in no particular order, so the picker shows which file it took as Left and which as Right, and **⇄ swap** exchanges them. Fletcher doesn't read the ears from the file names.
- You name the headphone; the files are stored as `measurements/<name>/L.txt` and `R.txt` (keeping each file's own extension).

**How each ear is used:**

| | Shown on the graph | Used by A target… |
|---|---|---|
| Ears not edited separately | Both ears averaged | The average: one chain for both ears |
| Each ear edited separately (Settings › EARS) | Each ear's own file, with that ear's EQ | Each ear fitted on its own: a preset with a left and a right chain |

The average is of sound level, not of dB: two ears at −1 and −3 dB average to about −1.9 dB, not −2.

**What a measurement is, and isn't:** one rig's average over a few seatings, not your ears. From 6 kHz up (7 kHz for in-ears), peaks and dips move with fit and ear shape, so the graph shades that range, fully from 10 kHz, and labels it "less reliable".

## Targets

A target is the response you're aiming for. Settings › TARGETS lists AutoEQ's featured targets, searches all of AutoEQ's (61 files: Harman over-ear 2013/2015/2018, in-ear 2016–2019, diffuse field, rig-specific variants…), and holds your own.

**Import a file:** *import…* takes one response file and a name. It's stored as `targets/<name>.txt` with `targets/<name>.json` beside it, and shown exactly as the file is. Fletcher then asks whether to add bounds (two more files, see below); you can also add them later in the editor.

**Make one:** *new target…* starts from a base (any target, or flat) and four knobs, AutoEQ's and squig.link's: bass (low shelf, 105 Hz, Q 0.7), ear gain (peak, 2.75 kHz, Q 1), treble (high shelf, 2.5 kHz, Q 0.42) and tilt (dB per octave, pivoting at 632 Hz). It's stored as `targets/<name>.json`: the base and the knob values, so it follows its base. The editor shows which rig the base was made for and, when AutoEQ has it, that rig's ear response (HRTF).

**Rig matters:** a target made on one rig (a GRAS, a B&K 5128, a 711 coupler) only means something next to a measurement from a similar rig. Fletcher names both rigs and lets you pair them; it doesn't convert between rigs.

## Preference bounds

A band around a target: how far from it listeners still liked the sound. Two files, the band's **upper** and **lower** edges, each as **offsets from the target** in dB (`+3` = 3 dB above the target at that frequency), in the response format above.

- Add them in the target's editor (Bounds › add…), or when importing the target. Pick both files at once, in either order: the file with the higher average is the upper edge.
- *replace…* takes a new pair; *remove* drops them. They're stored as `targets/<name> bounds upper.txt` and `… bounds lower.txt` and renamed with the target.
- On the graph, the band moves with its target (EQ view › Bounds, under the target).

There is no standard set. The bounds most people have seen (Headphones.com's, drawn on squig.link graphs) are built from Harman's research but carry no license, so Fletcher can't include them; if you have the files, import them. Fletcher's own, from Harman's published papers or measured with your own blind tests, are planned (ADR-0021).

## How the curves line up

Measurements, targets and bounds each come at their own level. Fletcher places them by shape, so the comparison means something:

1. **The measurement** is set to 0 dB on its average over the third-octave band around 500 Hz (IEC 60268-7's reference band; how Harman presents its targets). With two ears, the offset comes from their average and both ears move by it, so the difference between them stays.
2. **The target** is moved onto the measurement by the offset that fits it best (least squares) over 100 Hz–10 kHz. Only the target moves, never the measurement.
3. **Bounds** are added to the target after it's moved.
4. **Measurement with your EQ** is the measurement plus your EQ curve as drawn, without the preamp (a level, which level matching handles) and without refitting, so what you see is what the EQ does.

**Smoothing** (EQ view › Smoothing) applies to the measurement: none, 1/24, 1/12 (the default), 1/6 or 1/3 octave, or ERB (about one auditory filter wide).

## Presets from files

**REW filter export** (REW's EQ window › File › Export › Export filter settings as text, Generic equaliser): New preset… › REW file…. Read the way Equalizer APO would play the same file if you Included it:

- Commas are decimal points, and an Fc like `8.000` is 8000 Hz.
- `PK`, `PEQ` and `Modal` are peaks; `LP`/`HP` (and `LPQ`/`HPQ`) are second-order with Q 0.707 if none is given; `NO` without a Q is Q 30; the shelf variants (`LS`, `LS 6dB`, `LS 12dB`, `HS …`, `LSC`, `HSC`) are each resolved to the same filter with a center frequency and Q.
- What can't be reproduced exactly is skipped and named, never dropped silently: first-order `LP1`/`HP1` (APO's filters are all second-order), Linkwitz transforms, and bandwidth (`BW Oct`) filters (export with Q instead). The notice lists each skipped line and why.
- REW's own save file (`.req`) can't be read; use the text export.

**Filter lines** (Ctrl+V on the EQ page): Equalizer APO lines (`Filter: ON PK Fc 1000 Hz Gain -3 dB Q 1.41`, a whole preset's worth is fine) or a pasted REW export. Other lines are ignored.

**Another tool's chain** (New preset… › Peace's chain, or any other tool whose file is Included in APO's config): copies that tool's filters and preamp into a new preset, to edit as your own. Peace empties `peace.txt` while its EQ is switched off, so Fletcher reads Peace's saved state (`Last Configuration.peace`) then; it works with Peace on or off. Fletcher never changes the other tool's files.

**AutoEQ corrections** (type a headphone's name in the preset menu): AutoEQ's published parametric EQ for that headphone and measurement, fetched and cached, as a new preset.

**A target…** (New preset… › A target…): fits the playing output's headphone to the chosen target with AutoEQ's own method, ported to Fletcher and checked against AutoEQ's published results (within 0.06 dB on average up to 10 kHz for the HD 650 and Harman over-ear 2018):

- The error between measurement and target, smoothed at 1/12 octave, widening to 2 octaves from 6 to 8 kHz so fit-dependent treble detail isn't chased.
- Boosts capped at +6 dB, slopes limited to 18 dB per octave (except around real dips).
- Fitted with a 105 Hz low shelf and four peaks, then a 10 kHz high shelf and four more peaks on what's left: ten filters, rounded as AutoEQ prints them (Hz to 1, dB to 0.1, Q to 0.01).

The result is an ordinary preset, named after the headphone and the target, ready to edit.

## History files

History panel › import… reads a history file exported from Fletcher (`.json`): a preset's whole undo tree, every branch and snapshot. It replaces the current tree, and you land on its current node. *export…* writes one.

## Where it's all kept

Under `%APPDATA%\Fletcher\`:

| Path | What |
|---|---|
| `measurements/<headphone>/L.txt`, `R.txt` | Your imported measurements, unchanged |
| `targets/<name>.json` | Your targets (base and knobs, or the imported file's name, and any bounds) |
| `targets/<name>.txt`, `… bounds upper.txt`, `… bounds lower.txt` | Imported target and bounds files, unchanged |
| `presets/<name>.txt` | Presets, in Equalizer APO's own syntax |
| `autoeq/` | AutoEQ's indexes (refreshed weekly) and the files fetched from it (kept; used when offline) |
| `state.json` | Which headphone each output has, among other settings |

## Credits and licenses

AutoEQ's measurements index, targets and published results come from [AutoEQ](https://github.com/jaakkopasanen/AutoEq) (MIT, © 2018–2022 Jaakko Pasanen); the measurements themselves are by their measurers (oratory1990, Rtings and others), named with each one. Fletcher fetches them when you ask and caches them; none ship with the app. Data from squig.link, PEQdB and Headphones.com carries no license Fletcher could find, so it only reaches Fletcher as files you import yourself. The research behind these choices: [docs/research/targets-and-preference-bounds.md](research/targets-and-preference-bounds.md).
