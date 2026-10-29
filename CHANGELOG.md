# Changelog

Each version's section becomes its GitHub release notes and the "what's new" text in Settings › UPDATES. Write for listeners, not developers.

## 0.3.0

- **A/B comparisons are matched much more accurately.** Fletcher now judges loudness the way loudness meters (LUFS) do, so presets with treble boosts are no longer under-counted. When one side can't reach your reference level without clipping, the other side now comes down with it, instead of one side playing quieter. If two settings truly can't be matched, Fletcher says so rather than calling them matched.
- **In Clip Studio, A/B is measured on the song itself.** Playing through your EQ, each flip measures both sides on the part of the song you're hearing, and the louder one comes down to match. Even extreme EQs now flip at equal loudness. The A/B bar shows "matched (measured on this song)".
- **No more sudden volume jumps.** Any rise in level, from a reference change, a preset switch or leaving a comparison, fades in over about a second, never more than 3 dB at a time. Large typed increases ask first.
- **Blind tests got harder to "cheat".** Switching between A and B in clip tests no longer briefly gets louder mid-switch, and clip trials correct each clip on its measured loudness.
- **A finer frequency grid** with labels at 20, 50, 100, 200, 500… and a graph that uses the whole panel. Thanks for the request (#4)!
- **Custom graph scale.** The scale options are now auto, ±6, ±12, ±20 and custom. Custom lets you set the top and bottom separately with sliders or by typing; let go of a slider near its end and it re-centers, so you can keep going.
- **A real icon.** Headphones with orange ear cups over a waveform that fades away into the distance, in the window, taskbar, tray and installer.
- **Opening Fletcher twice brings the existing window forward** instead of starting a second copy.
- Fletcher fades in briefly at launch.

## 0.2.2

- Clearer wording throughout. Tooltips, Settings notes and messages now speak plain language instead of referring to internal design notes, and show tool names (Fletcher, Peace, Equalizer APO) instead of config file names.
- Release notes in Settings › Updates are now formatted properly, starting with the next update.

## 0.2.1

- **Import your Room EQ Wizard filters.** In REW's EQ window choose File › Export › Export filter settings as text, then in Fletcher's preset menu click **From REW file…**. The filters arrive as a new preset, read exactly the way Equalizer APO would play them: European number formats, Modal filters, and every shelf type included. Anything Fletcher can't reproduce exactly (first-order or Linkwitz filters) is listed by name, never dropped silently. Pasting a REW export with Ctrl+V works too. Thanks for the request (#1)!
- **Shelf filters now draw where you hear them.** Equalizer APO reads Fletcher's low- and high-shelf filters as corner-frequency shelves, so their curves were drawn up to about 1 dB off. Level matching in A/B tests was computed from that slightly wrong curve, and clip-based blind tests played the shelf at the wrong frequency. All three now match what APO plays. Your presets sound exactly as before; only the curve and the auto preamp move a little.
- **Copying filters from Peace or other tools keeps their sound.** Filters without a written Q (a bare notch or shelf) used to change slightly when Fletcher took them over. Now they don't.
- Q values show three decimals, and very deep EQ curves stay inside the graph instead of drawing over the axis labels.

## 0.2.0

- **Fletcher now updates itself.** It checks quietly in the background. When a new version is out, a dot appears on the SETTINGS tab, and Settings › Updates shows what's new and installs it in one click. Updates are cryptographically signed, and they never install during a blind test or a hearing-profile session.
- **"My EQ does nothing" is now explained and fixable.** Windows updates and audio-driver reinstalls can silently remove Equalizer APO from your playback device, leaving every change inaudible. Fletcher now detects this, tells you which device is affected, and one button opens APO's device selector to fix it. The device menu also marks devices without APO.
- **Output trim.** A new Settings › Output control raises or lowers Fletcher's overall level (−20 to +12 dB) without breaking level-matched A/B comparisons. It's for when another tool or a hand-edited config line has left everything too quiet. Fletcher also tells you how much attenuation comes from lines it isn't allowed to touch.
- **If you're on 0.1.0:** install this version by hand from the release page. It's the first one that can update itself, and every update after it arrives inside the app.

## 0.1.0

- First release.
