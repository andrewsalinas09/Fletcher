# Equalizer APO channel selection (for ADR-0017)

Source: the official [configuration reference](https://sourceforge.net/p/equalizerapo/wiki/Configuration%20reference/), read 2026-10-30. The source code wasn't reachable that day (the SourceForge code tree and the one GitHub mirror tried both returned 404). Nothing below has been tested against live APO yet.

## Documented
- **Syntax:** `Channel: <position> <position> …`. Positions are given "by identifier (acronym of 1 up to 3 characters) or by number (counted from 1)". "Multiple channels can be specified by separating with space. The special position "all" selects all channels." Examples on the page: `Channel: L RL`, `Channel: 1 2 C`.
- **Identifiers:** L, R, C, LFE (subwoofer; not "SUB"), RL, RR, RC, SL, SR. On stereo, L = 1 and R = 2. "If a channel configuration is not supported, channels can only be selected by number."
- **Scope:** "Selects the channels to which the following Filter and Preamp commands will be applied." Preamps that apply to the same channel sum in dB (since 0.8).
- **Device:** if the pattern doesn't match, "all following commands except Device commands are ignored".

## Not documented (so Fletcher doesn't rely on it)
- *Measured 2026-10-31 (apo-scope-and-loopback.md): it does not; `Channel:`, `Device:` and `Stage:` selections end with their file.* Whether a `Channel:` (or `Device:`) selection made inside a file pulled in with `Include:` persists after the include returns. The page's top-level example (`Channel: L`, then `Include: demo.txt`, commented "Filters only for left channel") suggests a selection made *before* an Include applies inside it. It says nothing about restoring one made inside. **Fletcher ends a split fletcher.txt with `Channel: all`** (TB-45).
- Case sensitivity of identifiers. Fletcher writes uppercase and `all`, as the docs do, and reads leniently.
- What happens to a filter while the selection names only positions the device lacks (C on stereo). Fletcher's readers treat such filters as reaching neither ear (`eq_state` tags them "none").
- Whether `Copy:` honors the channel selection.

## To verify live (when convenient)
On this machine (APO 1.4.2): a split fletcher.txt with a strong left-only cut, then listen or measure each ear. Also check what a parent `Channel: L` before `Include: fletcher.txt` does to an unsplit Fletcher chain.
