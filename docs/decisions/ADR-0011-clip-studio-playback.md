# ADR-0011 · Clip Studio playback: ffmpeg-native decode, managed tools, and the three play methods

**Status:** Accepted 2026-08-23 (user rulings in-session)

## Context

Clip Studio needs an in-app track engine (ADR-0009 gave it the exclusive-mode
bypass and the config-blank fallback). Building it surfaced three decisions:
how to decode, how to ship external tools, and — the important one — what
playback *is for* in the curation room.

## Decision

**1. ffmpeg is the native decode path.** Every source decodes through
`ffmpeg -f f32le -ac 2 -ar <rate>` into a per-(track, rate) PCM cache —
one canonical format, every container/codec, resampling included (TB-07
resolved by construction). symphonia (the earlier Path-2 sketch) is out: it
lacks Opus (exactly what yt-dlp sources serve) and duplicates what ffmpeg
does better. First decode also measures the track's **flat LUFS, once,
stored in the library DB** — replays hit the cache and start instantly.

**2. Managed tools, fetched on demand (ADR-0008 precedent).** ffmpeg and
yt-dlp are downloaded with explicit consent into `%APPDATA%\Fletcher\tools\`
(PATH is honored first), never bundled, never system-installed. GPL binaries
invoked as separate processes keep the Apache-2.0 codebase clean — and as a
non-commercial open-source app the pairing is unproblematic regardless.
yt-dlp URL import is committed scope. Settings shows every tool's resolved
path (TOOL PATHS).

**3. Three play methods; curation BYPASSES by default.**
- **Bypass (default):** curation studies the *material*. Exclusive device,
  no EQ, no APO — the track itself, level-matched toward the reference
  (target −16 LUFS at the default −8 reference, scaling with `referenceDb`;
  attenuate-only, TB-06) using the stored flat LUFS. No A/B exists here —
  nothing to compare — so no new A/B semantics and no EQ-panel inconsistency.
- **Through your EQ (opt-in):** a regular media player. Shared path — APO
  applies the chain and the level-matched config-swap A/B exactly as for any
  other stream. Zero special cases by construction.
- **Testing (Lab, phase ③):** the dual-bus in-engine machinery (both buses
  always processed through identical buffering, ~15 ms equal-power
  crossfade, true-LUFS trim — TB-12-clean sample-accurate blind switching).
  Built and dormant; it wakes when blind trials over clips need it. This is
  the only place that complexity earns its keep.

**4. Session discipline.** One session at a time; a new play *waits* for the
previous stream's fade-out and device release (streams never overlap), and
every session carries a serial so a superseded session's end can't clobber
its successor's UI. Every stream opens with the mandatory ramp and closes
with a fade (TB-20, engine-enforced).

## Consequences

ARCHITECTURE's Path 2 is corrected (ffmpeg, not symphonia). The disk grows
`tools/`, `cache/pcm/` (~23 MB per track-minute, purgeable), and
`clips/library.db` (the deliberate SQLite exception to the human-readable
rule). Exclusive-mode eviction of other apps is accepted as inherent to
bypass. TB-08's config-side shortfall is surfaced in the A/B bar
("matched — short by X dB"); in-engine matching can always reach a perfect
match by trimming B.
