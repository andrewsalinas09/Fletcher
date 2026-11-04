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
2. Text and strokes keep their chroma, and their lightness is solved so the APCA contrast against the dark paper equals today's against the light paper. APCA, not WCAG 2, because WCAG 2 misjudges light-on-dark.
3. Surfaces and lines keep their OKLab lightness step from the paper. Raised stays lighter and recessed darker; lines flip so they still stand out.
4. Accents on a selected (inverted) cell are matched against that cell, the ink.
5. Washes keep their step from the ground with the hue locked, and their chroma is raised 1.49× for the Hunt effect (CAM16's F_L^¼ between the two grounds). Measured first: a 14% sRGB blend of the orange over near-black is L 0.28, C 0.03, and 14° yellower, which is brown.

6. **Roles, not values.** In light, the ink does two jobs: text and marks (thin, small) and selection fills (tabs, segments, current rows: large blocks). Darkest is right for both. In dark they split. A large bright block on a dark ground glares and dominates (irradiation: bright areas on dark look bigger and brighter), which is the old "invert colors" look the owner spotted. So selection has its own tokens: `--sel` / `--sel-ink`, equal to the ink and paper in light, and in dark a raised neutral (`#45423b`, L 0.38) with ink text on it (APCA 86, light 97). The accents and knob lines on a selected cell are re-derived against that fill. Small state marks (slider tracks and thumbs, progress dots, a switch that's on) stay the ink.

Only the dark ground (L 0.21), the selection fill's lightness and the shadow and scrim alphas were chosen by eye.

**Mechanics:**
- Every color is a token in two blocks at the top of App.css (light `:root`, dark `:root[data-theme="dark"]`). Nothing below them writes a color.
- `src/theme.ts` resolves Light / Dark / Match Windows, sets `data-theme` before the first render, sets the Windows title bar, and syncs all windows (localStorage plus a `theme-changed` event). The default is Light.
- Canvas reads tokens with `tok()` / `tokA()`, cached until the theme changes, and redraws on `useThemeTick()`.
- `scripts/check-colors.mjs` runs in CI. It fails on a color literal outside the token blocks or in a .ts/.tsx string, and on a token with no dark value. Exempt: `csp-watch.ts` (the alarm must draw without the stylesheet) and `theme.ts` (builds rgba from tokens).
- The spectrogram heat map (ink to orange to paper) is data color, the same in both themes.

## Consequences

- A new color is a new token with both values, or CI fails. A feature can't ship light-only.
- The dark orange is lighter than the first draft (`#ff9661`): matching contrast at that lightness reaches the sRGB gamut edge. If small orange text ever reads too pale, the principled alternative is matching contrast only for strokes and using a deeper orange for small text.
- The derivation scripts are kept with the session's work, not in the repo; the rules above are enough to redo them.
