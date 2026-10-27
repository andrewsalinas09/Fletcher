# How Equalizer APO actually interprets a `Filter:` line

Source: Equalizer APO's C++ source, read 2026-10-26 from the GitHub mirror
`mirror/equalizerapo` (last synced 2024-09-28; the shelf/Q rules below date from
1.2.1, older than the installed 1.4.2): `filters/BiQuadFilterFactory.cpp`
(parsing, defaults), `filters/BiQuadFilter.cpp` (corner→center conversion),
`filters/BiQuad.cpp` (RBJ coefficients). The configuration reference
(sourceforge.net/p/equalizerapo/wiki/Configuration reference) gives the type
table but not these rules. **APO is ground truth for what the user hears, so
anything Fletcher draws, level-matches or imports must reproduce these rules.**

## Numbers
- **Every comma in a Filter line's parameters becomes a dot** before parsing.
  European REW exports (`Fc 50,0 Hz`, `Gain -10,0 dB`) are therefore *audible*.
- **Fc only:** if the number string is ≥ 5 chars, has no `e`/`E`, and its 4th
  char from the end is `.`, it's multiplied by 1000 (`8.000` → 8000 Hz,
  `10.000` → 10000). The comment says "because of Room EQ Wizard". Non-breaking
  spaces (another thousands separator) are stripped. Gain and Q get no such rule.

## Types
- `PK`, `PEQ`, `Modal` → the same peaking biquad. Modal's `T60 target` is
  ignored; REW has already turned it into the written Q.
- `LP`/`LPQ` → low-pass, `HP`/`HPQ` → high-pass. **Q is honoured if written**
  for either spelling (default 1/√2 if absent). Gain is ignored.
- `BP` (default Q 1/√2), `NO` (**default Q 30**, "found out by experimentation
  with RoomEQWizard"), `AP` (Q required).
- `None` is silently skipped (REW's empty slots).
- `BW Oct x` replaces Q for non-shelves: α = sin ω · sinh(ln2/2 · bw · ω / sin ω).
  This is sample-rate dependent, so no fixed Q is equivalent.

## Shelves (`LS`, `HS`, `LSC`, `HSC`)
The shape is the RBJ shelf with A = 10^(gain/40). Which frequency and steepness:

| Written | Steepness | Frequency is |
|---|---|---|
| no Q, no slope (`LS Fc 300 Hz Gain 5 dB`) | S = **0.9** | center |
| `… Q x`, type **without** trailing C (`LS`, `HS`) | Q = x | **corner** |
| `… Q x`, type **with** trailing C (`LSC`, `HSC`) | Q = x | center |
| slope `N dB` after the type (`LS 12dB`, `HS 6dB`, `LSC 6dB`) | S = N / 12 | corner unless trailing C |

- S-form: α = sin ω / 2 · √((A + 1/A)(1/S − 1) + 2). That makes **S = 1 identical
  to Q = 1/√2**, and gives the equivalent Q = 1 / √((A + 1/A)(1/S − 1) + 2).
- Corner → center: if Q-form, first S = 1 / ((1/Q² − 2)/(A + 1/A) + 1); then
  f_center = f_corner · 10^(|gain| / 80 / S) for a low shelf (÷ for a high
  shelf). The code calls it a "frequency adjustment for DCX2496".
- `LS 6dB` is **not** a first-order filter: it's the same two-pole biquad with S = 0.5.

## Consequences for Fletcher (2026-10-26)
- Every REW Generic type maps *exactly* onto Fletcher's PK / LPQ / HPQ / BP /
  NO / AP / LSC / HSC with explicit Q, because APO and Fletcher both read
  `LSC/HSC … Q` as center-frequency RBJ shelves. That's the REW importer (issue #1).
- **TB-34 (fixed 2026-10-26, see TESTBED):** Fletcher writes `LS/HS … Q 0.707` and drew it as a center-frequency
  shelf. APO plays it as a corner-frequency shelf, shifted by 10^(|gain|/80)
  (×1.19 at 6 dB). Fletcher also forces Q = 1/√2 when drawing LP/HP/LS/HS even
  when another Q is written, and draws a Q-less `NO` from someone else's file
  with Q 0.707 instead of 30.

## REW 5.31.3 text export (local experiment, 2026-10-26)

A real export (Generic equaliser, one of every type) is the fixture `crates/fletcher-core/tests/fixtures/rew_5_31_generic.txt`. Compared with the 2013 sample (`demo.txt`), REW now writes some names APO can't read. APO's type regex takes letters up to the first space or digit:

| REW 5.31 writes | APO does | Fletcher import |
|---|---|---|
| `LP Q`, `HP Q`, `LS Q`, `HS Q` | reads `LP`/`LS`…, then the Q, so it works (LS/HS with Q = corner shelf) | same as APO |
| `Notch`, `Notch Q` | "Invalid filter type", **silent** | `NO` (Q 30 if absent): REW's intent |
| `All pass` | invalid type, **silent** | `AP`: REW's intent |
| `LP1`, `HP1` (first-order) | reads `LP`/`HP`, so a **second-order** filter, not what REW designed | skipped with a reason |
| `L-T` (Linkwitz transform, Fp/Qp) | invalid type, silent | skipped with a reason |

REW's own save format (`.req`, "Save filter settings") is Java object serialization (magic bytes `AC ED`, class `roomeqwizard.Filter`), not text. Only REW reads it, and the importer says so.
