# Testbed

Every known edge case, captured the moment it's raised — before it's solved. Every design change gets checked against this list. Statuses: **Open**, **Addressed by <design/ADR>**.

**Prime directive:** Fletcher must never damage the user's existing audio setup. Failure modes may lose Fletcher's own state — never the user's config, and never their hearing (no surprise full-volume output).

---

- **TB-01 · APO absent.** Equalizer APO isn't installed, or is installed but not enabled for the selected device. Fletcher must detect both and guide, not silently no-op. *(Open)*
- **TB-02 · Peace owns the config.** User has an existing Peace (or hand-written) `config.txt`. Fletcher must not clobber it — coexist or migrate with explicit consent, and be cleanly uninstallable back to the prior state. *(Open — Q-03)*
- **TB-03 · Double processing in track mode.** Fletcher's own playback through the normal device path gets APO-processed on top of the in-app DSP. *(Open — Q-04)*
- **TB-04 · Exclusive-mode sources bypass APO.** ASIO / WASAPI-exclusive apps (audiophile players, some games) skip APO entirely; user concludes "EQ is broken." Detect/explain. *(Open)*
- **TB-05 · Device vanishes or renames.** USB DAC unplugged mid-session; Windows renames a device; config targets a device that no longer exists. *(Open)*
- **TB-06 · Clipping from positive gain.** Boost filters without preamp compensation clip digitally. Auto-preamp must account for filter *interaction* (summed response peak), not just the max single gain. *(Open)*
- **TB-07 · Sample-rate/format mismatch in track engine.** 44.1 kHz file, 48 kHz device; 24-bit; multichannel files. *(Open)*
- **TB-08 · Level matching hits the rails.** Matching a heavily-boosted config to bypass requires attenuation past comfortable listening, or gain past headroom. Define behavior + messaging. *(Open — Q-06)*
- **TB-09 · External config edits while Fletcher runs.** Peace, a text editor, or another tool writes the config Fletcher is watching → reload loops or silent divergence from UI state. *(Open)*
- **TB-10 · Misleading small-N statistics.** 3/3 correct feels conclusive and isn't (p = 0.125). Stats display must not let users fool themselves — that's the whole point of the product. *(Open — Q-05)*
- **TB-11 · Crash during a bypass window.** If track mode temporarily blanks/modifies the device config (Q-04b) and Fletcher crashes, the user's EQ must come back on next boot/launch. Restore must be crash-proof (journaled/atomic). *(Open — Q-04)*
- **TB-12 · Blind test unblinds itself.** Any observable asymmetry between A and B — switch latency, crossfade audibility, UI tell, residual level difference — invalidates blinding. *(Open)*
- **TB-13 · Clip battery picks bad clips.** Auto-extracted "bass-heavy" clip is actually a fade-out, cuts mid-phrase, or isn't representative — the whole systematic test inherits the error. Needs quality gates + manual override. *(Open — Q-11)*
- **TB-14 · Reinsertion variance in mic measurements.** Re-seating an in-ear mic or headphone shifts the measured response by several dB in the treble — easily larger than the differences being measured. Measurement mode must average multiple reseatings and report variance, or its "statistically identical" claim is false. *(Open — Q-12)*
- **TB-15 · Preference stats treated like discrimination stats.** A preference vote has no correct answer; reporting it with ABX-style "you scored 12/16" framing misleads. The two protocols need visibly different result presentations. *(Open — Q-05)*
- **TB-16 · Inverse-filter blowup.** Deconvolution (HpTF matching, HRTF removal) naively inverts deep nulls into huge narrow boosts — audible ringing, clipping, possibly speaker/ear damage at high volume. Regularization and boost caps are mandatory. *(Open — Q-12, Q-13)*
- **TB-17 · The stock preset isn't neutral.** "Level-matched to everything" assumes the flat reference is actually bypass-equivalent (preamp 0, no filters). If the reference itself drifts (e.g. inherited preamp), every comparison is silently biased. *(Open — guarded by ADR-0003)*
- **TB-18 · Bridging overreach.** A bridged fingerprint (reviewer's library → user's ears via a shared headphone) is trustworthy mainly below ~5–6 kHz; presenting the full curve with uniform confidence invites false treble conclusions. Per-frequency confidence must be computed and *shown*. *(Open — Q-15)*
- **TB-19 · Incompatible fingerprints silently combined.** Libraries captured with different mic/rig types (in-ear mic vs. coupler, different smoothing) get bridged as if comparable. Interchange format must carry capture metadata and Fletcher must refuse or down-weight mismatched bridges. *(Open — Q-15)*
