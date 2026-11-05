# ADR-0022 · The preset palette: a list, and "New preset…"

- **Status:** Accepted (2026-11-04). Resolves Q-33. Mockups: claude.ai/artifact/TJwR6U4jfyzznBTA4Xu7U3 (the owner chose C, with A's search and A's target panel).

## Context

The preset palette had grown four always-on icons per row (rename, settings, duplicate, delete), and making a preset was spread over three places: an external tool's row with a copy button, the AutoEQ section, and a name field with three buttons. "From target…" (ADR-0021 step 6) would have added a fourth. The owner: "It's honestly really bad."

## Decision

- **The list is what the palette is for.** One search over your presets and AutoEQ; then Flat, YOUR PRESETS (name, its hotkey, ⚙ on the row under the mouse, the keyboard row and the current one), and AutoEQ's matches once you type (with who measured each; picking one imports it, as before).
- **The last row is "New preset…".** It turns the palette into the list of sources, each with one line saying what it does: Empty, What's playing, A target… (when the fit ships), REW file…, and each external tool's chain with its state. Esc or ‹ goes back.
- **A preset's own actions live in its settings** (ADR-0019's modal): Name, Hotkey, Duplicate, Delete. Delete asks once, in place, since it can't be undone.
- **A new preset with a placeholder name** (Empty, What's playing, a copy, a duplicate) opens its settings with the name selected, ready to type over. AutoEQ and REW imports are named by their source and don't.
- **External tools' rows leave the list.** What they contribute is already on the preset chip ("Fletcher + Peace"); copying their chain is a source under New preset….
- The palette is its own component (`src/presetmenu.tsx`), placed by App, so it can be redesigned alone.

## Consequences

- Making a preset takes one more step (New preset…, then the source). Accepted: it's far rarer than switching.
- "A target…" opens its panel inside the source page (A's design), added with the fit.
