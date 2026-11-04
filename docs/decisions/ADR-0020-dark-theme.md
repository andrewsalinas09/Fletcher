# ADR-0020 · A dark theme derived by rule, and colors that can't miss it

- **Status:** Accepted (2026-11-03)
- **Extends:** ADR-0010 (visual direction). The light theme is unchanged.

## Context

A user asked for dark mode. The owner wanted it "like this color but dark", not an inversion, and wanted the dark colors justified by color science, not by taste. Dark modes usually decay because colors are scattered through the code and new ones get added for one theme only.

## Decision

**The look:** the same paper and ink with the lights down. A warm brown-black ground, ink the color of the paper, orange and blue still meaning boost and cut.

**The light theme does not change.** Every new token's light value is exactly the color it replaced (checked by script for every replacement, 2026-11-03). Two tokens that were used but never defined (`--line-soft`, `--ink-dim`) were resolved to what they actually rendered.

**Dark values are derived from the light ones, in OKLCH:**
1. Hue lock: every color keeps its light hue (boost 46.7°, cut 251.4°); neutrals share the paper's 84.6°.
2. Accents (boost, cut, ok, warn, error, the right ear, comparison colors) keep their chroma, and their lightness is solved so the APCA contrast against the dark paper equals today's against the light paper. APCA, not WCAG 2, because WCAG 2 misjudges light-on-dark. Neutral text (ink, dim, ink-2, ink-3) gets 92% of the light contrast (ink APCA 87 instead of 95), which eases the glare of near-white on near-black.
3. **Neutrals keep their saturation, not their chroma.** How tinted a color looks follows chroma relative to lightness. The first version kept chroma, which at a quarter of the lightness is about four times the saturation: the grays read brown ("coffee", not "paper"). Lines and surfaces keep their own saturation; neutral text and marks take the paper's.
4. Surfaces and lines keep their OKLab lightness step from the paper. Raised stays lighter and recessed darker; lines flip so they still stand out.
5. Accents on a selected cell are matched against that cell.
6. Washes keep their step from the ground with the hue locked, and their chroma is raised 1.49× for the Hunt effect (CAM16's F_L^¼ between the two grounds). Measured first: a 14% sRGB blend of the orange over near-black is L 0.28, C 0.03, and 14° yellower, which is brown.

7. **Roles, not values.** In light, the ink does two jobs: text and marks (thin, small) and selection fills (tabs, segments, current rows: large blocks). Darkest is right for both. In dark they split. A large bright block on a dark ground glares and dominates (irradiation: bright areas on dark look bigger and brighter), which is the old "invert colors" look the owner spotted. So selection has its own tokens: `--sel` / `--sel-ink`, equal to the ink and paper in light, and in dark a raised neutral (`#4b4a48`, L 0.41) with ink text on it. Switches follow the same split: dim tracks, and the knob carries the state (dim off, bright on). The accents and knob lines on a selected cell are re-derived against that fill. Small state marks (slider tracks and thumbs, progress dots, a switch that's on) stay the ink.

Chosen by eye, by the owner from four rendered variants (2026-11-03): the ground (L 0.25, lighter than the first version's 0.21), the 92% text contrast, the selection fill's lightness, and the shadow and scrim alphas. The variant with a deeper shadow and a hairline highlight for depth was not chosen.

**Mechanics:**
- Every color is a token in two blocks at the top of App.css (light `:root`, dark `:root[data-theme="dark"]`). Nothing below them writes a color.
- `src/theme.ts` resolves Light / Dark / Match Windows, sets `data-theme` before the first render, and syncs all windows (localStorage plus a `theme-changed` event). The default is Light.
- The Windows title bar is forced dark only in Dark. Light and Match Windows leave it to Windows, as before: the owner keeps a dark title bar over the light theme.
- Canvas reads tokens with `tok()` / `tokA()`, cached until the theme changes, and redraws on `useThemeTick()`.
- `scripts/check-colors.mjs` runs in CI. It fails on a color literal outside the token blocks or in a .ts/.tsx string, and on a token with no dark value. Exempt: `csp-watch.ts` (the alarm must draw without the stylesheet) and `theme.ts` (builds rgba from tokens).
- The spectrogram heat map (ink to orange to paper) is data color, the same in both themes.

## Consequences

- A new color is a new token with both values, or CI fails. A feature can't ship light-only.
- The dark orange is lighter than the first draft (`#ff9966`): matching contrast at that lightness reaches the sRGB gamut edge. If small orange text ever reads too pale, the principled alternative is matching contrast only for strokes and using a deeper orange for small text.
- The derivation scripts are kept with the session's work, not in the repo; the rules above are enough to redo them.
