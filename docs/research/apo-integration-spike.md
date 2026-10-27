# Research: APO integration spike (live, on the dev machine)

*Started 2026-08-22, Equalizer APO 1.4.2 + Peace installed the same evening. Findings verified by direct experiment unless marked pending.*

## Verified

- **Registry:** `HKLM\SOFTWARE\EqualizerAPO` → `InstallPath` = `C:\Program Files\EqualizerAPO`, `ConfigPath` = `...\config`, plus `EnableTrace`. Discovery is one registry read.
- **No elevation needed for config work:** the APO installer grants `BUILTIN\Users` **FullControl** on the config directory. A non-elevated process created `fletcher.txt` and appended to `config.txt` successfully. Fletcher requires admin for nothing in normal operation.
- **Peace's wiring convention:** `config.txt` contained exactly `Include: peace.txt`; Peace preserved the pre-install state as `configbeforePeace.txt`. ADR-0007's include-line pattern is the ecosystem convention, not an invention.
- **Live wiring done:** `config.txt` is now `Include: peace.txt` + `Include: fletcher.txt` (backup: `config.txt.before-fletcher.bak`); `fletcher.txt` is a comment-only no-op.
- **Real grammar sample:** Peace's HD650 AutoEQ preset uses `Device: all` / `Channel: all` / `Preamp: -8.1 dB` / `Filter N: ON <LSC|PK|HSC> Fc <f> Hz Gain <g> dB Q <q>` — the exact Phase 1 parser subset, with fractional Fc/Gain/Q values.
- **Peace behavior note:** Peace zeroes `peace.txt` (0 bytes) when its EQ is toggled off and rewrites the full block when on — external-edit churn Fletcher's file watcher must tolerate (TB-09).

## Verified 2026-08-22 (listening test, HD650)

- **Hot-reload through included files: works, fast.** `Filter: ON LP Fc 500 Hz` written to `fletcher.txt` (an included file, not `config.txt`) applied audibly and immediately; 4 on/off cycles at 1 s intervals all landed cleanly — listener verdict: "very fast", "basically 0 latency." In-place `Set-Content` writes were picked up fine; atomic rename-into-place remains the write strategy for robustness, not necessity.
- Implication: system-wide A/B on a hotkey is viable exactly as designed (ADR-0007 wiring + file toggle). Sub-second perceived latency at 1 s cadence; finer-grained latency measurement deferred until the ABX engine needs it (TB-12 cares about switch-time symmetry more than absolute speed).

## Verified 2026-08-22 (Q-04 bypass probe, `src/bin/spike_bypass.rs`)

Protocol: `Filter: ON LP Fc 500 Hz` active in `fletcher.txt`; 3 kHz tone at −24 dBFS played 4 s via WASAPI shared, then 4 s via WASAPI exclusive. Device: Schiit Modi 3+ (exclusive negotiated 24-bit int @ 48 kHz; shared f32 @ 48 kHz).

- **Exclusive mode bypasses Equalizer APO — confirmed.** Shared tone was strongly attenuated (filter applied); exclusive tone "much louder" (filter bypassed).
- **Exclusive mode evicts other apps, ungracefully.** Apple Music playing at the time cut out with an "error playback" dialog and did not auto-recover — user must manually restart playback after a session. Track-mode UX must warn before entering exclusive and can't promise other apps resume.
- **Exclusive bypasses Windows volume** (tone level = raw DAC output into the amp). The track engine MUST implement its own gain stage and start conservatively (TB-20). It needs its own volume control anyway for level matching, so this is alignment, not extra cost.
- Format negotiation worked on the first candidate list (32f/24i/16i @ 48k, 16i @ 44.1k) with the aligned-period retry path untested (default period succeeded).

Candidate decision (→ ADR once confirmed): exclusive mode is the primary track-mode bypass — zero config mutation (no TB-11 crash-restore hazard, no races with Peace's file watcher), naturally silences competing audio during a listening test — with a config-blank fallback only for devices where exclusive init fails.

## Verified 2026-10-26 (per-device registration, `examples/apo_devices.rs`)

Symptom: every Fletcher edit inaudible although `config.txt` included `fletcher.txt` and the file was being written. Cause: **APO was registered on no playback device at all.**

- APO is per-device. Its installer/`DeviceSelector.exe` writes one of APO's COM classes into the endpoint's effect slots at `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Render\{endpoint-guid}\FxProperties`. The classes, as registered under `HKLM\SOFTWARE\Classes\CLSID` on this machine: `{EACD2258-FCAC-4FF4-B36D-419E924A6D79}` "EqualizerAPO Pre-Mix Class", `{EC1CC9CE-FAED-4822-828A-82A81A6F018F}` "EqualizerAPO Post-Mix Class".
- The Modi 3+'s FxProperties held only Microsoft's defaults (`{C18E2F7E-933D-4965-B7D1-1EEF228D2AF3}` in slots `{d3993a3f-…},5/6`). None of the five active render endpoints named an APO class. The APO install itself (`HKLM\SOFTWARE\EqualizerAPO`, the DLL, the CLSIDs) was intact, so "APO is installed" was true and told us nothing.
- The endpoint registry key is the GUID after the last `.` of the MMDevice id (`{0.0.0.00000000}.{guid}`; `0.0.0` = render).
- APO 1.4.2's per-device tool is **`DeviceSelector.exe`** (older releases: `Configurator.exe`). It also leaves `backup_<device>.reg` files of the slots it replaces in the install dir.
- Likely trigger: a Windows feature update or audio-driver reinstall rewrites FxProperties (not reproduced here; inferred from the slot contents being stock).
- `PKEY_AudioEndpoint_Disable_SysFx` (`{1da5d803-d492-4edd-8c23-e0c0ffee7f0e},5`, mmdeviceapi.h) is the documented per-device "disable all enhancements" switch. It's read from the endpoint's `Properties` key. When it's 1, no effects load. Not observed set on this machine.
- **No reboot needed (1.4.2).** Ticking the Modi in DeviceSelector made the EQ audible immediately, without restarting Windows (user-verified live 2026-10-26).
