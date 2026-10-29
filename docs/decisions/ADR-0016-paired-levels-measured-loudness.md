# ADR-0016: Comparisons are paired, loudness is K-weighted, and levels change safely

- **Status:** Accepted. Supersedes ADR-0003's "0 preamp" / "matched to one fixed reference" wording, the estimator in Q-06 v2, and ADR-0012's "equal-power" crossfade. ADR-0003's intent (honest, level-matched comparisons by default) is unchanged.
- **Date:** 2026-10-28
- **Source:** A user's bug report (A/B unmatched at reference 0 dB); a first independent review; then an accuracy review by a second reviewer ([report](../research/loudness-matching-review-2026-10-28.md)), which found that the first version of this ADR over-claimed, and a [follow-up](../research/loudness-matching-followup-2026-10-28.md) (R1–R6). A third, independent review judged proportionality and found the scope had outgrown the goal. Owner decisions: "fix everything"; Clip Studio option 2 (normalize songs); then "this is supposed to be straightforward": keep the core, cut the rest (the plain-path rule; Bypass is not a comparison).

## Context
Level matching normalized each chain to the reference on its own: preamp = reference − (mean of the chain's dB curve), capped for clip safety. That broke in several ways:
- **Per-side clip cap.** At reference 0 dB, a +6 dB peak played several dB quieter than Flat (the bug report).
- **A biased estimator.** Against real LUFS on pink noise, the mean of dB under-counted treble boosts by up to 1.6 dB.
- **Volume jumps.** Matching off put Flat at 0 dB; a typed value applied at once; a positive trim could clip.
- **Exclusive playback** ignored Windows volume.
- **Unpaired paths:** previews, re-listen, and calibration noise.

The first fix then introduced its own errors, all caught in review:
- Clip Studio's EQ mode double-counted outside gain (~10 dB too quiet).
- The A/B crossfade was +3 dB mid-switch on correlated audio, an ABX cue.
- Plus a sparse peak scan, a reachable floor overriding the ceiling, scope-blind outside preamps, double-counted hardware volume, NaN trims on silent clips, and an unramped first write.

## Decision
1. **Loudness = K-weighted pink power** (`dsp::loudness_db`): the LUFS change a chain causes on pink noise.
   - Validated within 0.03 dB of ebur128 on rendered pink noise (`tests/loudness_vs_lufs.rs`).
   - It is an *estimate* for real programme: material that isn't pink, such as a 60 Hz tone through a bass shelf, can differ by several dB. The UI says "matched (estimate)" for system-wide comparisons and "matched (measured)" for clip trials.
2. **One level plan per chain** (`dsp::plan_level`):
   - target = reference + trim. The preamp puts the chain's loudness, relative to outside filters on the same path, at the target.
   - The **clip ceiling** is Fletcher's chain's peak plus the outside preamp that provably applies (§6). It is found by `dsp::response_peak_db` (log grid + every filter's Fc, refined by golden-section search) and **applied last**, so the floor can never push above it.
   - Shortfall > 0 means held below the target; < 0 means infeasible. A chain with no finite loudness (a filter at Nyquist silences everything) is infeasible at the floor; filter frequencies are clamped below Nyquist anyway.
3. **Every comparison is paired** (`dsp::pair_levels`):
   - Both sides come down by the larger shortfall. If equality is impossible, the pair reports `matched = false`, and the A/B bar and trials say unmatched.
   - Used by system A/B, both trial kinds, history previews (pop-out included), the labeled re-listen and the diff plot. Curves carry preamp + shortfall, so every curve difference is the paired one.
4. **Clip trials correct on the measured audio** (`dsp::measured_trims`):
   - B takes the correction; if B's headroom can't, A comes down by the rest, leaving no residual.
   - Silent or unmeasurable clips are refused.
   - Verified on rendered engine output (`app/src-tauri/tests/levels_rendered.rs`).
5. **Equal-gain switching:** the 15 ms crossfade is 1−x / x. A and B are the same audio through two EQs (correlated), so the gains must sum to 1; equal-power summed to √2.
6. **Outside path: plain or not** (`outside_path`). Fletcher does not model other tools' processing. It reads config.txt and the files it includes directly, and asks one question: is the path *plain* — nothing but `Preamp:`, `Include:` and comments, all readable?
   - Plain: outside Preamp cuts provably apply here, so they count as headroom (your `Preamp: -10 dB` line lets the trim and reference use those 10 dB).
   - Anything else (filters, GraphicEQ, Convolution, Device/Channel scopes, nested includes, a missing file): no outside cut counts. Fletcher's chain stays clip-safe on its own, and at worst the bar says "below reference". It errs quiet, never loud.
   - Outside Preamp *boosts* always count against the ceiling.
   - The config watcher (recursive) re-plans whenever these numbers change, and re-applies a running system-audio trial's current audition.
7. **Clip Studio (owner's choice):** every song is normalized to −16 LUFS (attenuate-only) in **both** play methods.
   - **Through your EQ** is the comparison mode, and here Fletcher has the actual audio, so it doesn't rely on the estimate. Each A/B flip (`apply_side`) measures both sides on what's being heard (the loop when one is on, else the 20 s around the playhead), read from the decode cache (`studio_measured_diff`), and **the louder side comes down** to match; nothing is raised. The correction goes on **Clip Studio's own stream** (`TrackShared::raw_gain_bits`, slewed over ~15 ms), never into fletcher.txt: it affects only the song, ends with the session, and leaves the system-wide level and the fade limiter on the plain estimate. (A first version put it in fletcher.txt; a review found it outlived the song, leaked into other apps, and fought the pink-level fade limiter.) It's synchronous, at the moment of the flip: nothing changes mid-song. After an EQ edit the stream keeps the last correction (quieter only) until the next flip re-measures. Level matching off → no measuring. The bar says "matched (measured on this song)". This matters most for strongly tilted EQs: a +28 dB treble shelf reads +18 dB louder on pink noise but only a few dB louder on music, and the estimate alone left Flat 4–13 dB louder across the owner's 13 cached songs.
   - **Bypass** is for studying the song itself; there is nothing to compare (ADR-0011 §3). It plays at a safe level that recreates the shared path's Flat: the paired Flat preamp, *every* outside Preamp cut whatever its scope (an unreadable file costs a 10 dB pad), and the *software* Windows volume, which exclusive mode skips. Hardware volume and mute are checked independently; each counts only if it's software. An unreadable volume is assumed to be −20 dB.
   - A first attempt (2026-10-28) measured in a background thread and rewrote the level mid-song; it looped and was removed. Measuring at the flip has none of that machinery.
   - Previews report when a pair can't be matched. Both graphs (main and pop-out) compute each pair's played difference with the same rule as `pair_levels`, including the unmatched fallback.
8. **Level changes are bounded** (`write_chain_safely`, the only writer of fletcher.txt):
   - A rise over 3 dB *of the modelled level* becomes a fade (`fade_steps`): at most 12 writes ~80 ms apart, each rising by the same 1–3 dB, so no write rises more than 3 dB. A rise over 36 dB starts 36 dB below. The first write of a run fades in from 12 dB below. A non-finite preamp is never written.
   - Typed increases over 3 dB ask first. Matching off keeps Flat at the reference.
   - The trim can't exceed outside headroom, and Fletcher re-plans whenever the outside path changes (§6).

## Why
- **Pairing, not a global "fits everything" level.** Pairing costs nothing in the common case and stays matched in the rare one. The cost: Flat's absolute level depends on what it's compared with, which the A/B bar shows.
- **K-weighted pink.** It's what LUFS measures, so the system and clip paths agree, and it's closed-form. A perceptual model for non-pink programme (ISO 532, or calibration by ear) remains open.
- **Normalizing songs in Clip Studio** keeps Flat ↔ EQ and song ↔ song fixed. The alternative, recreating the shared path's level exactly, depends on brittle endpoint-volume emulation and makes the level vary between songs.
- **Safety in the writer, not in each control.** No new path can forget it.

## What is and isn't guaranteed
- **Guaranteed, and tested:**
  - The plain-path rule: a plain cut is headroom; filters, GraphicEQ, scopes, nested includes and missing files earn none; exclusive playback always takes every cut (real-file tests in `lib.rs`).
  - A silent chain never produces a non-finite plan, and fades are bounded.
  - Paired preamps are equal by the estimate.
  - Measured clip trims leave < 0.05 dB residual on rendered output.
  - Switching identical buses never exceeds the input.
  - The steady-state magnitude response stays ≤ 0 dB on the whole modelled path.
  - Impossible matches are reported, not hidden.
  - Fletcher's own writes never rise more than 3 dB of modelled level in one step.
- **Not guaranteed:**
  - Perceptual equality on arbitrary music.
  - Inter-sample or transient true peaks.
  - The behavior of APO features Fletcher doesn't model. On a non-plain path Fletcher only guarantees its own chain stays ≤ 0 dB; what other tools add is theirs.
  - Trim headroom on complex APO configs (the bar shows "below reference" instead).
  - Level changes made outside Fletcher.
  - Windows volume changes mid-song in exclusive mode (they apply at the next play).

## Consequences
- Trial records store `pairBelowDb`, `levelMethod`, and per-clip `trimADb` / `trimBDb`.
- Calibration noise (pink, amplitude 0.2) measures −25.6 LUFS, about 17 dB below a typical modern master and 9.6 dB below Clip Studio's −16 LUFS. Calibrating the knob on it and then playing unnormalized music lands louder than calibrated. Open: align the noise with the programme level.
