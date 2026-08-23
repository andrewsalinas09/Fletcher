# Vision

> **Status: DRAFT** — captures the founding conversation (2026-08-22). To be rewritten after the idea-mapping sessions; until then treat it as directionally right, not normative.

Equalizer APO is a superb DSP engine whose entire control surface is a text file — and every existing frontend for it is either abandoned (AQUA), niche, or actively hostile to use (Peace). Fletcher is the modern frontend: all the power, none of the 2009 UI, plus a category of feature no consumer EQ tool has ever shipped — **honest listening tests**.

## North-star goals

1. **The most user-friendly system EQ ever made.** A newcomer gets from download to "my headphones sound better" without reading anything. Power features exist but never clutter the first-run path.
2. **Honest A/B — the killer feature.** Any two configurations can be compared blind (ABX), **level-matched by loudness (LUFS)** so "louder sounds better" cannot masquerade as "EQ sounds better," with real statistics: trials, correct count, binomial p-value, session history over time. Two modes:
   - **System-wide:** whatever is playing (Spotify, a game) — instant config swap on a global hotkey, riding APO's hot-reload.
   - **Track mode:** import a song, set in/out loop points, switch seamlessly (sample-accurate crossfade, shared playhead) between EQ'd and bypass — via an in-app audio engine, because system playback would be double-processed by APO (see ARCHITECTURE).
3. **Instant everything.** APO hot-reloads its config on file change; Fletcher exploits that everywhere — every knob is live, no Apply buttons, no audio glitches.
4. **A ton of features, discoverable not crowded.** AutoEQ headphone database, auto preamp/clipping protection, response-curve editor with draggable filters, presets, per-device profiles, and more (the full list is being mapped — see OPEN-QUESTIONS).
5. **Plays nice with the ecosystem.** Never destroys an existing Peace/manual config; imports what people already have.

## Why "Fletcher"

The Fletcher–Munson equal-loudness curves are the foundational research on how perceived loudness varies with frequency — the exact phenomenon (loudness bias) that Fletcher's level-matched testing is built to defeat.

## Why now

Equalizer APO's architecture (config-file API + hot reload) makes the frontend cleanly separable, the incumbents are abandoned or dated, and the UI surface — curve editors, stats dashboards, preset browsers — is exactly what modern tooling builds fast.

## What this is not

- Not a DSP engine — Equalizer APO does the system-wide processing; Fletcher drives it. (Fletcher's in-app engine exists only for track-mode A/B, replicating the same filter math.)
- Not a Peace clone with a fresh coat of paint — the listening-test loop (match → blind test → statistics → belief) is the product's spine, not an add-on.
