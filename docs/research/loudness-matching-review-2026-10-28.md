# Loudness-matching accuracy review — 2026-10-28

This is the original review snapshot. For the re-review after fixes, see [the follow-up](loudness-matching-followup-2026-10-28.md).

**Verdict: the current implementation does not support an unconditional “equally loud / never clips / never jumps” guarantee.** Pairing is the right repair for the original reference-ceiling bug. K-weighted power also substantially improves the old mean-of-dB estimate on pink noise. Several independent playback and boundary problems remain.

Scope: `380f300` (trim), `b087750` (APO filter semantics), `b9d205c` (paired comparisons), and the pre-existing uncommitted ADR-0016 rewrite. Findings below distinguish new regressions from inherited problems. Runtime source and the user's audio configuration were not changed in this review.

## Verified numerical results

The offline [diagnostic source](loudness-review-probe.rs) calls the actual DSP and `TrackSource` implementation, without opening an audio device. Its header gives the reproduction command. Six diagnostic assertions passed; these assert the observed defects, not the desired behavior.

| Case | Observed result |
|---|---|
| +8 dB LSC, 105 Hz, Q 0.7; 60 Hz input; reference −8 | System pink estimate gives preamps −9/−8 dB; actual measured A/B difference **6.1123 dB** |
| +12 dB LSC, 1 kHz, Q 0.7 versus Flat; 0.9999-amplitude 60 Hz input; reference 0 | Measured trim needs 5.2987 dB but is capped at 5.0009; **0.2979 dB mismatch remains** |
| Identical 0.8-amplitude buses during an A→B transition | Peak **1.131371**, a **3.0103 dB** increase; exceeds full scale before output conversion |
| +12 dB PK, 765.1327 Hz, Q 100 | Auto-preamp only −3.4 dB; actual center-frequency response **+8.6 dB** |
| Three +30 dB PK filters, 1 kHz, Q 1; target −50 dB | Floor forces A to −80 dB, Flat stays −50; **40.8815 dB mismatch**, reported shortfall 0 |
| Three seconds of silence | Both loudness measurements −∞; subtraction produces **NaN trim** |

## Findings

### F1 · P1 · Pair the measured correction too

`app/src-tauri/src/lib.rs:4382–4391`. Inherited from `9d02770`; neither `b9d205c` nor the working rewrite fixes it. After the pink-based pairing, the clip path measures actual LUFS, then raises only B subject to B's peak cap. When that cap binds, A is not lowered. The 0.2979 dB reproduction above still receives `level_matched = true`; the shortfall goes into saved provenance but never into `TrialState` or the visible warning. Subsequent common stream attenuation preserves the mismatch, even if it creates ample unused headroom. Solve the measured A/B gains together and verify the final processed signals; do not retain a “matched” status for a residual beyond the chosen tolerance. TB-08 remains open here.

### F2 · P1 · Apply bypass compensation only to paths that bypass it

`app/src-tauri/src/lib.rs:2956–2961`. New in the uncommitted rewrite. The `else` branch is Studio **EQ/shared** playback, but now adds `exclusive_offset_db()`, including endpoint volume, foreign preamp and trim. Those controls already affect that shared stream. At Windows −20 dB, with zero trim/foreign gain, this adds an erroneous further −20 dB. The “bypass requested, exclusive refused” fallback also receives exclusive compensation despite using shared mode. Branch on the actual output path; shared EQ should not receive that duplicate offset.

### F3 · P1 · The switching envelope supplies an ABX cue

`app/src-tauri/src/engine.rs:228–231`. Inherited from `4dc45f0`. Sine/cosine equal-power weights sum to √2 at the midpoint for identical correlated audio. The offline source test produces +3.0103 dB and >1.0 samples. The playback conversion clamps samples, so insufficient downstream attenuation also turns this into distortion. A→X makes this transition only when X is the other bus; selecting the already-active bus does not. Nearly identical permitted chains therefore have an artificial switching cue. The no-difference test rejecting exactly identical responses does not solve the correlated-audio case. The envelope and headroom policy need an explicit resolution under TB-12/Q-26; this review does not silently supersede ADR-0012.

### F4 · P1 · A 400-point response scan is not a peak bound

`crates/fletcher-core/src/dsp.rs:306–312`. Pre-existing, now relied on by the new whole-path ceiling claim. The valid +12 dB/Q 100 filter above falls between scan points: the planner misses 8.6 dB of gain. The existing “never clips” test checks the same sparse grid as the implementation, so it cannot catch this. Locate/refine response maxima independently and test against dense or analytic references over supported filter parameters. A steady-state magnitude bound also does not by itself bound arbitrary transient or reconstructed true peaks; the guarantee must state what is actually bounded.

### F5 · P1 · Moving the preamp floor does not eliminate the floor bug

`crates/fletcher-core/src/dsp.rs:393–399,405–409`. The new −80 dB floor reduces the frequency of the inherited lower-rail issue but remains reachable: `chain_of` permits ±30 dB per filter and imposes no filter-count limit. Three coincident +30 dB filters suffice. Clamping upward can exceed the ceiling; because shortfall only records downward misses, this is reported as fully matched. Handle infeasible plans explicitly, reject unsafe chains, or derive bounds that really cover the accepted domain. Never silently clamp a gain above its safety ceiling. TB-08/TB-06.

### F6 · P1 · Foreign preamp totals are not proof of available headroom

`app/src-tauri/src/lib.rs:592–593,3325–3357`. The existing reporting helper became safety-critical in the rewrite. It sums parsed Preamp lines in the root and direct includes, without evaluating Device/Channel scope or nested includes, and without incorporating outside filters. A −12 dB line scoped to a different device can consequently authorize +12 dB on a path where that attenuation does not exist. A nested positive preamp is omitted. Equalizer APO explicitly has device- and channel-scoped commands and included configuration files ([official configuration reference](https://sourceforge.net/p/equalizerapo/wiki/Configuration%20reference/)). This conclusion follows from that documented grammar and the inspected reader; it was not tested against the live APO installation. Grant compensating headroom only when the effective processing path is known; otherwise report the uncertainty conservatively. TB-36.

### F7 · P2 · Exclusive volume handling must distinguish hardware and software

`crates/fletcher-core/src/devices.rs:28–49`; `app/src-tauri/src/lib.rs:641–642`. New in the rewrite. The helper assumes exclusive mode always bypasses endpoint volume. Microsoft documents that hardware endpoint controls affect **both** shared and exclusive streams; only software endpoint controls are bypassed ([EndpointVolume API](https://learn.microsoft.com/en-us/windows/win32/coreaudio/endpointvolume-api)). On hardware-controlled endpoints the new stream gain therefore applies volume twice. Query hardware support and use the selected stream endpoint consistently. The current `None → 0 dB` fallback also provides no attenuation when the query fails, and the sampled volume does not track subsequent changes. These latter cases need an explicit fail-safe/session policy; no live endpoint behavior was exercised. TB-37.

### F8 · P2 · Absolute LUFS normalization does not reproduce shared playback level

`app/src-tauri/src/lib.rs:2957–2958,4349,4391`. The absolute target predates this rewrite; adding endpoint offsets does not make its new “same level as shared” claim true. Example: Flat, reference −8, no trim/foreign processing, software endpoint volume −20, source −30 LUFS. Shared output is approximately −58 LUFS (source −30 plus reference −8 plus volume −20). Bypass targets −36 and attenuates the source by 6, producing −36 LUFS: **22 dB higher**. This is algebra from the current paths, not a physical measurement. A fixed programme normalization target and emulation of existing shared-path gain are different contracts. Choose and expose one; do not promise equality between them. TB-37/Q-26.

### F9 · P2 · Reject clips without finite loudness

`app/src-tauri/src/lib.rs:4382–4389`. Inherited. A silent clip passes the duration checks, returns −∞ from both meters, and creates NaN trim. `TrackSource` then computes a NaN gain; final conversion is not valid measured playback. Reject silence and below-gate/unmeasurable material before creating a matched trial, and validate all resulting gain values. TB-38.

### F10 · P2 · Label the system estimate as an estimate

`app/src/App.tsx:6697,8076`; `crates/fletcher-core/src/dsp.rs:340`. The unconditional equal-loudness UI language is not supported by the algorithm. The accepted pink-spectrum approximation is not actual programme analysis: the 60 Hz example differs by 6.1123 dB despite the pink test being accurate. Keep ADR-0016's model, but qualify system comparisons and reserve a measured label for actual analyzed material with verified residual tolerance. Even the standard explicitly describes measured loudness as an estimate of subjective loudness with listener/material/condition uncertainty ([ITU-R BS.1770-5, page 2](https://www.itu.int/dms_pubrec/itu-r/rec/bs/R-REC-BS.1770-5-202311-I!!PDF-E.pdf)). TB-39/Q-06.

### F11 · P1 · The first write bypasses the new ramp

`app/src-tauri/src/lib.rs:320–350,5333`. New ramp implementation. `LAST_WRITTEN_LEVEL` starts at `None`; startup calls `apply_side`, which immediately writes the final level without comparing against the chain currently on disk. Restarting while a quieter preview/previous chain remains active can therefore bypass the claimed 3 dB step limit. Initialize from the effective existing state or use a conservative startup transition. Also, the variable measures modeled pink loudness, not actual programme loudness, so F10 limits any general acoustic “never jumps” claim. These are source-trace findings, not live restart experiments. TB-20 remains open.

## What passed, and what remains unverified

- `cargo test --workspace`: **60 passed, 2 hardware-dependent tests ignored**. The app crate has no unit tests covering the matching orchestration.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `npx tsc --noEmit` in `app/`: passed.
- The existing six-case seeded pink-noise validation reproduces its approximately 0.03 dB agreement. It supports that fixture set, not all accepted filters, signals, sample rates, or routing configurations.
- Preview partner data is wired through HistoryTree → PopoutHistory `hist-cmd` → main listener → `preview_chain`; wildcard capabilities cover satellites. The diff window includes shortfall too. This is static verification only; the user must still exercise the pop-out as required by repository rule 10.
- No audio playback, config writes, hearing test, or live restart was performed. Actual APO output, real endpoint volume behavior, true-peak behavior, non-48-kHz paths, and perceptual equality remain unverified.

Recommended acceptance work: fix F1–F9/F11; make F10 truthful; then add independent end-to-end rendered-buffer checks for final A/B LUFS residual, peak behavior, switches including A→A versus A→B, silent clips, both gain rails, device routing and actual sample rates. A numerical tolerance can be verified; “100% perceptually equal” cannot be certified by these tests.
