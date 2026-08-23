# Feature Map

The full feature vision, captured from idea-mapping sessions. Tiering into v1-spine / fast-follow / someday is pending (Q-07) — right now this is the honest wishlist, organized. Source: founding brain-dump 2026-08-22 (user), lightly structured.

## The first ten minutes (target onboarding flow)

1. Download → pick your output device.
2. Load an EQ preset (AutoEQ for your headphone) or make your own.
3. A **stock/flat reference preset** exists from the start, and everything is **level-matched against it by default** — so step 4 is honest out of the box.
4. Toggle it on and hear the difference.
5. Play a song and A/B between them.
6. Enter **blind mode**: the app switches for you and asks which is which / which you prefer.
7. Or: import a song, the app plays **clips** of it and asks which you like more.

## Core EQ

- Parametric curve editor: draggable filters, and the **individual filter curves shown against the summed response** they produce.
- Presets: create, save, load; AutoEQ headphone database import.
- Auto-preamp / clipping protection (from summed response peak, TB-06).
- Device selection and **per-device profiles**, with device-level auto-switching: Windows reports USB DAC connect/disconnect and default-device changes, so "DAC appears → its profile activates" is real. What's *not* detectable is which headphone hangs off a DAC/amp's analog jack — so headphone-level switching is a fast manual switcher (tray menu / hotkey), not magic.
- **Per-channel EQ and L/R balance**: independent left/right curves (APO supports per-channel filters natively). Serves asymmetric hearing — an accessibility win — and the Fingerprint Lab wants per-ear data anyway. Config engine and data model are per-channel-aware from day one.
- **Preset history as a branching tree**: every edit journaled (ADR-0006 provenance applied to tuning); generous Ctrl-Z stack; an optional keybinding forks a new *branch*, and the full tree is visible and jumpable. Combined with the test machinery: blind A/B any two nodes — "did my last hour of fiddling actually improve anything?" (Q-17)
- **Loudness-compensation mode (the namesake feature)**: EQ gently reshapes with listening level per the Fletcher–Munson/equal-loudness contours. **Off by default**, opt-in. Volume anchoring design: Q-16.

## The Listening Lab (the killer feature cluster)

- **Level matching everywhere.** Not just in tests — preset-vs-flat and preset-vs-preset comparisons are loudness-matched (LUFS) by default. (Leaning: matching on by default app-wide → Q-06.)
- **System-wide A/B**: hotkey flip of the live APO config; whatever you're playing.
- **Track mode**: import a song/movie audio, set cut-in/cut-out points, seamless in-app switching.
- **Two distinct test types**, both first-class:
  - **Discrimination (ABX)** — "can you actually hear a difference?" Binomial stats, p-values.
  - **Preference (blind A/B vote)** — "which do you *like*?" Forced-choice voting, preference stats.
- **Clip Studio + battery** (ADR-0004; vision expanded 2026-08-22): an NLE-grade curation room, DaVinci-Resolve-inspired. **Viewer**: waveform and spectrogram, large, swappable — one, the other, or both stacked sharing playhead/zoom/loop. **Timeline idiom**: in/out points (I/O keys), J/K/L scrub, markers (M), scroll-to-zoom around the playhead, timecode. **Annotation system**: point markers for moments, range clips tagged (lows/mids/highs/…), free notes, genre, arbitrary user tags — the track becomes a marked-up document. **Spectrogram box-select**: drag a time×frequency rectangle to isolate a moment's band (Q-18 moment isolation as a single gesture), solo it on loop, save it annotated with its frequency range. **Media pool**: searchable clip library across all tracks, filter by tag/genre/track; select clips → build a battery for the Lab. **Everything persists to the local database** (tracks, clips, tags, notes, markers, and the test results that reference them — ADR-0006 provenance). File tags (artist/album/genre) auto-imported, user-overridable. Auto-extraction still deferred.
- **Rich statistics** (user: "I love statistics"): per-test verdicts, per-clip/per-band preference breakdowns ("you prefer +bass on bass-heavy content, flat on mids"), history over time, per-headphone profiles (Q-05). **Hidable during sessions** — revealing interim stats carries a professional-language bias caution and is recorded in provenance (TB-24).
- **The Lab is the finder**: Clip Studio annotates ("add too much information"); the Listening Lab searches it — query the clip database by tag/genre/track/band/notes and assemble **custom batteries** on the spot. Curation and retrieval are separate rooms by design.
- **Labeled session replay**: every test stores exactly what played (trial order, X assignments, clips, matched levels); afterwards, replay the entire session with labels revealed — relive trial 7 knowing X was B.
- **Tuning × testing fusion**: move a slider and the app replays the clip battery (or the relevant clip) to highlight what that change does — tuning loop and test loop share machinery.
- **Moment isolation + similarity search** (Q-18): inside a loaded song, interactively sweep/filter to isolate *which frequencies make the part you love sound like that* — then search your library for material with a similar spectral character to test against. Turns "I like this, whatever it is" into a testable, named preference.
- **Agent hook (future, MCP)**: expose Fletcher's analysis as an MCP server so an AI agent can be asked "at 2:53 the voice sounds amazing — what is that?" and answer with real spectral data (and mark the moment as a clip). Ecosystem phase.

## The Fingerprint Lab (own tab, advanced tier — Q-12, Q-15)

Nothing on the market does this. Its own tab, visibly its own setup:

- **Capture**: guided "capture headphone fingerprint" flow — sweeps + multiple reseatings, measured at *your* ears. Each fingerprint is the coupled HpTF∘ear response; HpTF and HRTF aren't separated and don't need to be, because within one person's library the ear factor cancels out of comparisons.
- **Library**: fingerprints accumulate per headphone (HD650, HD800S, …); experiment freely — e.g. EQ one headphone to statistically match another, verify by re-measurement.
- **Export/import + bridging**: libraries are shareable files. Reviewers (headphones.com, The Headphone Show) capture in-app and publish; users import and Fletcher **bridges** via a shared headphone — (X_on_reviewer − shared_on_reviewer) applied to shared_on_you ≈ headphone X on your ears. At-home virtual auditioning of basically any headphone, honest about validity (reliable mainly below ~5–6 kHz; per-frequency confidence shown, TB-18).
- **Mic-free hearing profile ("the long game", Q-19)**: estimate your personal perceived frequency response with *no hardware at all* — band-limited noise pairs, loudness-matched by answering forced-choice questions, bands recursively subdivided (statistically driven, log/ERB-spaced, possibly overlapping bands to isolate narrow regions). Long and tedious by design; the statistics engine makes every answer count and shows convergence. Measures the perceptual sum (headphone × ear × hearing) at the test loudness — which is exactly the thing EQ acts on. Prior art (SoundID, Mimi) exists but closed, unstatistical, and unintegrated.
- **Mic calibration import**: calibration curves (frequency/dB pairs + sensitivity header) applied to all captures, with the cal file identity recorded in fingerprint metadata (TB-19). First-class formats: miniDSP (UMIK-series), Dayton iMM-6, OmniMic — real fixtures exist in the user's `AudioAnalyzer/Calibration Files`.
- Resolved: binaural recordings were the matching stack all along (Q-13); no separate playback feature.

## Scope note

**Headphones first.** Speaker/room correction (REW import, measured room EQ) is deferred — the config engine still round-trips `Convolution` and other directives losslessly (Q-09), so nothing is foreclosed.

## Visualization

- Live **spectrogram/waveform** of what's playing, showing how the curve shapes it.
- **Side-by-side spectrograms**: flat/default vs. your EQ — maybe a third with HRTF applied.
- **FFT pane** (Clip Studio viewer): instantaneous spectrum at the playhead with the current EQ curve overlaid, and the regions the EQ acts on lightly shaded — orange where it boosts, blue where it cuts — so the curve's effect on the live spectrum is visible at a glance.
- Filter handles + summed curve + (later) measured/target overlays on one graph.

## UX principles

- **It's for everyone.** One UI, not separate beginner/expert apps.
- **Progressive disclosure by greying, not hiding**: in standard mode the layout is *identical*, advanced features are slightly greyed; hovering explains what the feature does and why it's advanced. Discovery is built into the disabled state. (Leaning → Q-02; candidate ADR.)
- **Instant everything**: no Apply buttons; APO hot-reload makes every control live.
- **Pop-out windows, one truth** (Q-20): any tab can become its own OS window — curve on one monitor, Lab on another — and every window updates together in real time, because the Rust core owns all state and windows are just views.
- **Bidirectional editing**: the graph and the filter strip edit the same model — drag a handle or type a number, both update live.
- **Never lie about loudness**: the app treats unmatched comparisons as invalid by default.
- **Everything recorded, so everything is known** (ADR-0006): every test, measurement, and comparison stores full provenance — configs, levels, matching offsets, capture metadata. Results are auditable, never vibes.
