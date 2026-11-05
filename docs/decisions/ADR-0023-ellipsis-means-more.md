# ADR-0023 · "…" means there's more

- **Status:** Accepted (2026-11-04). Owner: "the ... shows there's more … it's quite clean. I actually _really_ like this ... meaning it continues. We should adopt it everywhere else it makes sense."

## Decision

A command's label ends in "…" exactly when clicking it asks for something more before anything happens: a file to pick or save, a choice from a list, a combination to press, an editor to fill in, or a confirmation. A command that acts at once has none.

- Yes: New preset…, REW file…, import…, export…, change…, assign… (waits for a combination), new target…, Delete… (asks once), set one… (opens the output's settings to pick).
- No: Duplicate, Make preset, save, remove, pop out, and the device hotkey's "assign", which takes the next free combination at once.
- Always the single character "…", never three dots.
- Separate uses that stay: progress ("checking…", "importing…") and placeholders ("search presets…"), which aren't commands.

## Consequences

- Applied across the app on 2026-11-04 (Clip Studio's + Import track…, + From link…, ∿ Generate…; the history's import…/export…; Settings' new target…; the target editor's change…; the EQ view's set one…; the preset hotkey's assign…).
- New labels follow it; review checks it like the color tokens.
