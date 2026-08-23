# ADR-0004: Clip libraries are user-curated, not auto-extracted (for now)

- **Status:** Accepted
- **Date:** 2026-08-22
- **Source:** Idea-mapping session (user: "Clip extraction is not automated for now. It's user selected… they load the audio… build a library for each thing… lows, highs, mids for _that particular sound track_… a whole database maybe sorted by genre")

## Context
The clip battery powers testing, tuning, and statistics — so clip quality bounds the quality of everything downstream. Automatic extraction risks unrepresentative clips (TB-13), and the user knows their music better than a band-energy heuristic does.

## Decision
Clips are selected by the user in a dedicated curation view: load a track, see its waveform and spectrogram, mark in/out spans, tag them (lows / mids / highs / etc.). Tagged clips accumulate into a personal library, organized per-track and sortable by genre or other metadata; the rest of the app consumes this library on demand. Automatic extraction is deferred — at most a future *suggestion* assistant inside the same curation view.

## Why
High-quality, personally-meaningful test data beats convenient test data — the statistics are only as trustworthy as the clips. Manual curation also sidesteps the whole TB-13 failure class at v1 cost near zero, since the waveform/spectrogram view is needed anyway. Rejected for now: DSP auto-picker as the primary path (quality gates and musical-coherence detection are a research project of their own).

## Consequences
- The curation view (waveform + spectrogram + span marking + tagging) is a core surface, not an advanced one.
- Needs a clip-library data model: track reference, time spans, tags, genre/metadata, creation date.
- Battery-driven features (systematic tests, tuning×testing fusion) are designed against the library API, so a future auto-suggester slots in without redesign.
- Q-11 is resolved; TB-13 becomes moot for v1 (revisit if auto-suggestion lands).
