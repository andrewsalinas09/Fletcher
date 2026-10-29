# Loudness review follow-up — 2026-10-28

**Verdict: improved, but not ready for an “all fixed” sign-off.** Reviewed the working tree after the “Every review finding fixed” WORKLOG entry; HEAD remains `b9d205c`. No runtime code or live audio configuration was changed by this review.

## Remaining findings

### R1 · P1 · An unrelated filter cut can still authorize unsafe gain

`app/src-tauri/src/lib.rs:656–668` (`walk_config` filter arm).

The revised scope handling only guards negative **Preamp** lines. Every filter is still combined into one cascade, even when assigned to another device or channel. Adding more filters is not necessarily conservative: a cut can cancel a boost that actually applies elsewhere.

Offline reproduction: `Device: OtherDevice`, a −12 dB PK at 1 kHz/Q 1, then `Device: all` and Fletcher's +12 dB PK at the same frequency. The modeled path cancels the filters and writes −1.3 dB; the actual selected path has **+10.7 dB** gain at 1 kHz. Separate L/R paths can fail similarly. Keep separate effective paths or decline to use uncertain attenuation as safety headroom or loudness context. The [official APO reference](https://sourceforge.net/p/equalizerapo/wiki/Configuration%20reference/) confirms Device selection gates commands and Channel selection assigns filters/preamp to particular channels. The numeric reproduction uses the current walker and core DSP, not live APO playback. Original F6/TB-36 remains open.

### R2 · P1 · Include traversal silently omits real processing

`app/src-tauri/src/lib.rs:639–656` (`walk_config`). Two independently reproduced failures:

- `Include: gain.txt` twice, with `Preamp: 6 dB` inside it, counts only +6 dB because `seen` is global for the whole traversal. The second include is not a recursive cycle. Fletcher writes −6 dB against actual +12 dB, leaving **+6 dB** unprotected.
- Root includes `sub/chain.txt`, which includes `gain.txt`, located at `sub/gain.txt` with +12 dB. The walker resolves all files against the root directory, misses the nested file and models **0 dB instead of +12 dB**.

APO's [IncludeFilterFactory source](https://raw.githubusercontent.com/mirror/equalizerapo/master/filters/IncludeFilterFactory.cpp) resolves relative names from the including file and processes repeated includes, guarding recursion with a depth counter. This is primary project source through the mirror already used for repo research; the installed binary was not exercised. Use resolved identities, the including file's directory and an active recursion stack; surface unreadable/depth-limited paths rather than treating them as known-safe. Original F6/TB-36 remains open.

### R3 · P1 · The watcher does not invalidate the new level model

`app/src-tauri/src/lib.rs:5298,5316–5329`; `foreign_preamp_db` at 3458.

Planning now depends on outside filters, scope and nested includes. Replanning still happens only when the old root/direct-include **preamp sum** changes. Turning an outside −12 dB filter OFF leaves the scalar unchanged, so an earlier preamp that relied on its attenuation stays in the file. A scope-only change likewise goes unnoticed; removing a nested negative preamp also leaves the legacy scalar unchanged. Subdirectory files are not watched (`NonRecursive`). UI refresh/`ab_info` only recomputes displays, not the written preamp, so a fresh-looking UI can disagree with playback. Invalidate from the complete effective path/dependencies and handle active-trial changes explicitly. Source trace only; no live config was changed. TB-36/TB-09.

### R4 · P2 · Studio bypass still ignores the comparison's capped level

`app/src-tauri/src/lib.rs:767–770` (`exclusive_listening_db`).

The shared double-count is fixed and both Studio methods now use the same song normalization. But exclusive Flat uses the **solo** Flat plan, while `apply_side` writes **paired** levels to shared EQ. For a +12 dB PK at 1 kHz/Q 1, reference 0, zero trim/foreign/endpoint gain, the paired Flat preamp is −8.5 dB; exclusive Flat uses 0 dB. The normalized input is identical, yet the model predicts bypass **8.5063 dB louder** than shared EQ. This persists even within the accepted pink-noise model, independently of its programme limitations. Use the active comparison's common level for Studio Flat. Original F8/TB-37 remains open.

### R5 · P2 · Hardware volume support does not establish hardware mute support

`crates/fletcher-core/src/devices.rs:50–56`.

The early return on `ENDPOINT_HARDWARE_SUPPORT_VOLUME` happens before reading mute. Hardware volume and mute have independent support bits. On an endpoint with hardware volume but software mute, Windows can be muted while this function returns 0 dB; exclusive playback bypasses the software mute. Check mute and its support independently before returning the bypassed gain. [Microsoft's QueryHardwareSupport documentation](https://learn.microsoft.com/en-us/windows/win32/api/endpointvolume/nf-endpointvolume-iaudioendpointvolume-queryhardwaresupport) specifies independent capabilities and software substitution; [EndpointVolume documentation](https://learn.microsoft.com/en-us/windows/win32/coreaudio/endpointvolume-api) explains exclusive-mode bypass of software controls. This is a conditional source/API finding, not a claim that the user's Modi exposes that combination. Original F7/TB-37 remains open.

### R6 · P2 · Infeasible previews and satellite graphs discard the unmatched result

`app/src-tauri/src/lib.rs:1171–1175`; `app/src/App.tsx:495,1965`.

The main A/B bar and trials now recognize `pair_levels(...).3 == false`. History/re-listen previews still take only `.0`, return `()` and communicate no unmatched status. Both graph paths still add `shortfallDb` as though pairing had succeeded. When `pair_levels` falls back to independent plans, this is no longer the audible difference.

Offline reproduction: four +30 dB PK filters at 1 kHz/Q 1, target −50, app range −120…+12. The pair correctly reports `matched=false`, but the graph's preamp offset is −100.1 dB versus the played −70.1 dB: **30 dB error**. Thread feasibility and the actual pair gains through preview responses, `hist-cmd`/sync and both main/pop-out renderers. Partner plumbing and wildcard capabilities remain intact, but they do not carry this new result. Original F5/TB-08 remains partially open; satellite parity is incomplete.

## Disposition of the original findings

| Original | Follow-up result |
|---|---|
| F1 measured trim rail | Fixed for the reproduced case; rendered correction test passes |
| F2 shared double-count | Fixed in actual-mode branching, by inspection |
| F3 correlated switching boost | Fixed for identical buses; rendered transition test passes; this does not certify no perceptible cue for arbitrary different chains |
| F4 missed narrow peak | Original case fixed; independent dense-grid tests pass; not a mathematical proof over all filter combinations |
| F5 floor/infeasible plans | Core/main/trial handling improved; previews and graphs remain incomplete (R6) |
| F6 outside path | Not fixed (R1–R3) |
| F7 endpoint handling | Hardware-volume duplicate fixed; independent mute remains (R5) |
| F8 Studio levels | Normalization mismatch addressed for uncapped plans; capped pair still diverges (R4) |
| F9 silent clips | Fixed; finite-value validation and rejection test pass |
| F10 estimate label | Main matching labels now distinguish estimated/measured; accepted programme approximation remains |
| F11 first write | Now initializes from disk/fades on read failure, by inspection; actual startup/failure behavior not exercised |

## Verification and scope

- `cargo test --workspace`: **68 passed, 2 hardware-dependent tests ignored**.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- `npx tsc --noEmit`: passed.
- Five additional offline diagnostics reproduced R1, both R2 cases, R4 and R6. [Runner](run-loudness-followup-probe.ps1) extracts the current production `walk_config` into a temporary integration test alongside [probe source](loudness-followup-probe.rs), runs it, and removes the temporary test. Its assertions document defects rather than desired behavior. Fixture files are created and removed only in the temporary directory.
- R3 and R5 are source traces with explicit triggering conditions. No live playback, APO mutation, endpoint change or restart was performed. User testing of Studio Flat/EQ, trial switching and history/diff pop-outs remains required after these issues are corrected.

The new rendered suite is useful but does not exercise `track_play`, `walk_config`, the config watcher, endpoint capability branching, or IPC propagation of infeasible previews. Those are the gaps these remaining failures expose.
