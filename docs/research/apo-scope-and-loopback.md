# Equalizer APO: scope across Include, and hearing APO through loopback

Measured 2026-10-31 on the dev machine: APO 1.4.2, Schiit Modi 3+ with both APO classes (pre-mix and post-mix), Windows 11 25H2 (build 26200), Peace 1.6.9.11 installed alongside. Probe: `crates/fletcher-core/examples/scope_probe.rs`. It plays a 1 kHz tone at −30 dBFS on the default output and records the same device through WASAPI loopback. For each case it rewrites fletcher.txt, a nested `probe-inc.txt` and peace.txt, waits 2.5 s, and reads the level per ear. It restores all files byte-for-byte afterwards.

## Results
Every case adds `Preamp: -20 dB` somewhere. "−20" means APO applied it; "0" means APO skipped it.

| # | Case | L | R |
|---|---|---|---|
| 1 | `Preamp: -20 dB` in fletcher.txt | −20.0 | −20.0 |
| 2 | `Device: NoSuchDeviceXYZ`, then the Preamp, in the same file | 0.0 | 0.0 |
| 3 | fletcher.txt: `Include: probe-inc.txt` (holding `Device: NoSuchDeviceXYZ`), then the Preamp | −20.0 | −20.0 |
| 4 | the same with `Stage: capture` inside the include | −20.0 | −20.0 |
| 5 | the same with `Channel: L` inside the include | −20.0 | −20.0 |

Cases 6 and 7 (a `Device:` or `Stage:` line at the end of peace.txt) were spoiled: the running Fletcher re-planned on the outside change and rewrote fletcher.txt mid-measurement. Cases 3–5 test the same boundary, an included file returning to its parent.

A separate run (`setup.rs` live test, config.txt temporarily edited) put `Device: Realtek Headphones` in config.txt before `Include: fletcher.txt`. A −20 dB change to fletcher.txt then measured 0.0 dB.

## What this establishes
- **WASAPI loopback records APO's output.** A config change shows up in a loopback recording to 0.1 dB. So Fletcher can verify its own path with no microphone (Settings › SETUP, ADR-0018).
- **`Device:`, `Stage:` and `Channel:` selections end with the file they're made in.** One made inside an included file doesn't carry back to the file that included it. So nothing in peace.txt can switch Fletcher off, and fletcher.txt's selections can't leak into config.txt (TB-45, our side).
- **A non-matching `Device:` line in config.txt above `Include: fletcher.txt` silences Fletcher completely** on that output. APO's docs say "all following commands except Device commands are ignored", and that includes the Include itself. So no line inside fletcher.txt can undo it.
- APO reloads a changed file within about 0.7–2.5 s here.

## Peace (from its help file, Peace.chm, version 1.6.9.11)
- Selecting a device in Peace's device list makes that configuration "only active on that specific device". With `Write Device GUID=1` (peace.ini), Peace writes `Device:` lines.
- Peace's Automation window can keep several configurations "Always active", e.g. one per device.
- Peace has an effects-panel option, "Equalization stage: before or after mixing".
- Peace's troubleshooting page recommends installing APO as SFX/EFX on Windows 10 and 11. If that doesn't work, it suggests SFX/MFX and then LFX/GFX ("which for some Windows 10 and 11 updates seems to be the best option").

## APO stages (configuration reference)
"Initially, the selected stages are post-mix and capture." Fletcher writes no `Stage:` line, so its EQ runs post-mix only. On an output where only APO's pre-mix class is installed, nothing Fletcher writes is heard. A config that selects `Stage: pre-mix` (Peace's "before mixing" option) is still heard there. Not yet reproduced on this machine: it needs APO reinstalled on the device with only pre-mix ticked.

## Still open
- Case sensitivity of `Device:` patterns. Fletcher's check assumes matching ignores case.
- Whether a parent's `Channel:` before `Include: fletcher.txt` applies inside it (TB-45).
- Whether loopback includes APO on every driver. If it doesn't, the live test says it couldn't measure rather than claiming a result.
