# ADR-0019 · Preset settings, starting with a hotkey per preset

**Status:** Accepted (2026-10-31)

## Context

A tester uses one output two ways (speakers and headphones) and wants to switch presets with a hotkey, as in Peace (Q-29). Devices already have a settings panel (⚙ in the device menu) with a hotkey that switches to the device (#10).

## Decision

- **Every preset gets a settings panel**, opened from ⚙ in the preset menu (between rename and duplicate) and from ⚙ beside the preset name on the EQ page (for the active preset). Its first setting is a hotkey; later per-preset settings go in the same panel.
- **No preset has a hotkey by default.** "assign" waits for a combination the user presses. Unlike a device's, no Ctrl+Shift+digit is picked for them (owner, 2026-10-31). Once assigned, it's the same field as Settings › HOTKEYS (remap, on/off, popup, reset to the assigned combination, remove), and it's listed there as "Load <preset>".
- **Pressing it loads the preset as picking it from the menu does**: rises fade in, and it lands on side A. The popup names the preset and the current output, or says APO isn't on that output. Not during a blind test (nothing happens, and no popup), and not during a hearing session (the popup says so).
- **Stored** in state.json `presetSettings` (name → { hotkey }), the action id `preset:<name>`. A rename moves the setting and registers the hotkey again under the new name. A delete drops it.

## Consequences

- The free-digit search is shared with device hotkeys (`assign_hotkey`); only devices use it.
- A "next preset" hotkey that cycles a list is still open (Q-29).
