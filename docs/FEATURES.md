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
- Device selection and per-device profiles.

## The Listening Lab (the killer feature cluster)

- **Level matching everywhere.** Not just in tests — preset-vs-flat and preset-vs-preset comparisons are loudness-matched (LUFS) by default. (Leaning: matching on by default app-wide → Q-06.)
- **System-wide A/B**: hotkey flip of the live APO config; whatever you're playing.
- **Track mode**: import a song/movie audio, set cut-in/cut-out points, seamless in-app switching.
- **Two distinct test types**, both first-class:
  - **Discrimination (ABX)** — "can you actually hear a difference?" Binomial stats, p-values.
  - **Preference (blind A/B vote)** — "which do you *like*?" Forced-choice voting, preference stats.
- **Clip Studio + battery** (ADR-0004): a dedicated curation view — load a track, see waveform + spectrogram, mark and tag clips (lows/mids/highs/…) — building a personal library per track, organized by genre. Tests run **systematically across the battery**: play all; A/B each; or randomized blind A/B per clip with voting. Auto-extraction deferred (future suggestion assistant at most).
- **Rich statistics** (user: "I love statistics"): per-test verdicts, per-clip/per-band preference breakdowns ("you prefer +bass on bass-heavy content, flat on mids"), history over time, per-headphone profiles (Q-05).
- **Tuning × testing fusion**: move a slider and the app replays the clip battery (or the relevant clip) to highlight what that change does — tuning loop and test loop share machinery.

## The Fingerprint Lab (own tab, advanced tier — Q-12, Q-15)

Nothing on the market does this. Its own tab, visibly its own setup:

- **Capture**: guided "capture headphone fingerprint" flow — sweeps + multiple reseatings, measured at *your* ears. Each fingerprint is the coupled HpTF∘ear response; HpTF and HRTF aren't separated and don't need to be, because within one person's library the ear factor cancels out of comparisons.
- **Library**: fingerprints accumulate per headphone (HD650, HD800S, …); experiment freely — e.g. EQ one headphone to statistically match another, verify by re-measurement.
- **Export/import + bridging**: libraries are shareable files. Reviewers (headphones.com, The Headphone Show) capture in-app and publish; users import and Fletcher **bridges** via a shared headphone — (X_on_reviewer − shared_on_reviewer) applied to shared_on_you ≈ headphone X on your ears. At-home virtual auditioning of basically any headphone, honest about validity (reliable mainly below ~5–6 kHz; per-frequency confidence shown, TB-18).
- **Binaural recordings**: possible companion feature (correct playback on any fingerprinted headphone); scope unresolved (Q-13).

## Visualization

- Live **spectrogram/waveform** of what's playing, showing how the curve shapes it.
- **Side-by-side spectrograms**: flat/default vs. your EQ — maybe a third with HRTF applied.
- Filter handles + summed curve + (later) measured/target overlays on one graph.

## UX principles

- **It's for everyone.** One UI, not separate beginner/expert apps.
- **Progressive disclosure by greying, not hiding**: in standard mode the layout is *identical*, advanced features are slightly greyed; hovering explains what the feature does and why it's advanced. Discovery is built into the disabled state. (Leaning → Q-02; candidate ADR.)
- **Instant everything**: no Apply buttons; APO hot-reload makes every control live.
- **Never lie about loudness**: the app treats unmatched comparisons as invalid by default.
- **Everything recorded, so everything is known** (ADR-0006): every test, measurement, and comparison stores full provenance — configs, levels, matching offsets, capture metadata. Results are auditable, never vibes.
