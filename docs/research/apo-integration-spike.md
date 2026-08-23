# Research: APO integration spike (live, on the dev machine)

*Started 2026-08-22, Equalizer APO 1.4.2 + Peace installed the same evening. Findings verified by direct experiment unless marked pending.*

## Verified

- **Registry:** `HKLM\SOFTWARE\EqualizerAPO` → `InstallPath` = `C:\Program Files\EqualizerAPO`, `ConfigPath` = `...\config`, plus `EnableTrace`. Discovery is one registry read.
- **No elevation needed for config work:** the APO installer grants `BUILTIN\Users` **FullControl** on the config directory. A non-elevated process created `fletcher.txt` and appended to `config.txt` successfully. Fletcher requires admin for nothing in normal operation.
- **Peace's wiring convention:** `config.txt` contained exactly `Include: peace.txt`; Peace preserved the pre-install state as `configbeforePeace.txt`. ADR-0007's include-line pattern is the ecosystem convention, not an invention.
- **Live wiring done:** `config.txt` is now `Include: peace.txt` + `Include: fletcher.txt` (backup: `config.txt.before-fletcher.bak`); `fletcher.txt` is a comment-only no-op.
- **Real grammar sample:** Peace's HD650 AutoEQ preset uses `Device: all` / `Channel: all` / `Preamp: -8.1 dB` / `Filter N: ON <LSC|PK|HSC> Fc <f> Hz Gain <g> dB Q <q>` — the exact Phase 1 parser subset, with fractional Fc/Gain/Q values.
- **Peace behavior note:** Peace zeroes `peace.txt` (0 bytes) when its EQ is toggled off and rewrites the full block when on — external-edit churn Fletcher's file watcher must tolerate (TB-09).

## Pending

- **Hot-reload semantics** (this spike's next step): audible verification that edits to an *included* file apply live; latency feel; behavior on rapid successive writes; atomic-rename vs. in-place write. Method: listener on HD650s, toggle a dramatic-but-safe filter (`Filter: ON LP Fc 500 Hz`) in `fletcher.txt`.
- **Q-04:** how track-mode playback bypasses APO (exclusive-mode vs. temporary device bypass vs. other) — separate spike, needs the audio engine probe.
