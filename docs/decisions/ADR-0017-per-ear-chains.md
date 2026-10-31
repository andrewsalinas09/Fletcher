# ADR-0017: A preset can have a chain per ear; one preamp keeps both matched

- **Status:** Accepted. The UI is Q-28 (v1 built 2026-10-30, shipping in 0.7.0).
- **Date:** 2026-10-30
- **Source:** Owner, 2026-10-30: "this is the next major feature update. Start by making it work in the backend. … each preset has separate L/R, default to the same, and you can unlock it in settings to add a button to the main EQ page, since most people won't use this and it would clutter everything." FEATURES.md has listed per-channel EQ since the first scoping session.

## Context
Equalizer APO applies `Filter:` and `Preamp:` lines to the channels named by the last `Channel:` line ([configuration reference](https://sourceforge.net/p/equalizerapo/wiki/Configuration%20reference/), [research note](../research/apo-channel-selection.md)). Fletcher wrote one chain for both ears. Its config readers ignored `Channel:` too, so copying a per-ear source (Peace, or a hand-written file) merged both ears' filters into one chain. Everything downstream also assumed one chain: the level planner, A/B, Clip Studio's engine, blind tests and the history inspector.

## Decision
1. **Data model:** `config::Chains { left, right: Option }`. `right: None` (the default, and every existing preset) plays the left chain on both ears. `Some` is a split preset. It stays split even while the two lists are equal, so the user, not a comparison, decides when the ears are joined.
2. **File format** (presets and fletcher.txt): unsplit is written exactly as before. Split is `Channel: all`, the preamp, then `Channel: L` + the left filters, then `Channel: R` + the right filters, and it ends with `Channel: all`. APO doesn't document how a selection crosses an Include (TB-45). So the opening `Channel: all` keeps a parent `Channel: L` from confining our preamp (and every fade step) to one ear while the other ear's filters still play, and the closing one keeps lines after our Include from depending on ours. Reading is lenient: `ConfigDoc::chains` follows `Channel:` (L/1, R/2, all, case-insensitive), and positions other than L and R reach neither ear. A file is split when a `Channel:` line selects exactly one ear (`selects_one_ear`), not when filters happen to land unevenly. So an empty split preset stays split, and a C/LFE-only section splits nothing. A Preamp on one ear only (a balance) is dropped from a preset's advisory preamp.
3. **One preamp for both ears.** Level planning (`dsp::plan_level_stereo`) uses the two ears' power together (LUFS sums channels, so it's the mean of the power gains) and the louder ear's peak for the clip ceiling. Linked chains give exactly the old numbers. Pairing, fades and trims are unchanged (ADR-0016).
4. **Both ears go everywhere a chain goes** (the UI paths since Q-28 v1, TB-48).
   - The engine's buses run a cascade per channel (`ChainProcessor::new_stereo`), and `lufs_peak_through` measures through both.
   - Blind tests take `aRight` / `bRight`, and records keep `aChainRight` / `bChainRight`, omitted when absent so old records read unchanged.
   - History previews, curves and promote-to-preset take optional right ears.
   - `eq_state` tags each filter with the ears it reaches, and adds `sumDbRight` (only when it differs) and `split`.
5. **Commands:**
   - `chains_split` splits the active preset (both ears start as its chain, so nothing audible changes) or joins it (keeping one named ear's chain).
   - `set_fletcher_chain(filters, right, preset, ear)` applies an edit to the named preset's saved chains, replacing only the named ear. It drops an edit for a preset that is no longer active, and refuses a split preset sent as one chain, so a caller can never silently merge two ears into one.

## Why
- **`Option` instead of two always-present lists:** nearly everyone uses one chain. Unsplit presets stay byte-identical on disk, readable by Peace and by hand, and every existing caller keeps working unchanged.
- **One preamp, not one per ear:** level matching compares sides at equal loudness for the listener, i.e. both ears together. Two independent preamps would be a balance control. That's a separate feature (FEATURES lists "L/R balance") and can be added later as an offset on top.
- **Rejected: "shared filters + per-ear extras".** Closer to how APO files are often written, but it makes every edit ambiguous ("is this filter shared?"). Two full lists match the owner's mental model ("separate L/R, default to the same").
- **Rejected: deriving split from L == R.** Splitting and then editing back to equal would silently re-join, and the UI's per-ear view would vanish.

## Consequences
- The UI (Q-28) edits one ear at a time as if it were a preset of its own, and every write sends both ears; a caller that sends one chain for a split preset is refused (TB-47) rather than merging the ears.
- Balance (a per-ear level) and per-ear hearing-profile data (Q-19 v2) can build on this model.
- A one-ear-only Preamp in an imported source is not carried into the preset (TB-46).
