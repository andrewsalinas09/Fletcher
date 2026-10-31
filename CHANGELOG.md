# Changelog

Each version's section becomes its GitHub release notes and the "what's new" text in Settings › UPDATES. Write for listeners, not developers.

## 0.7.0

- **A separate EQ for each ear.** Turn on Settings › EARS › Edit each ear separately, and every preset gets its own left and right curves, starting out identical. In the EQ page's ⚙, show one ear at a time, both on one graph, or the two side by side, and turn on "Enable ghost" to see the other ear faintly. Each ear keeps its own undo history. A/B, blind tests and history previews play both ears as they are, and stay level-matched across both together. Join the ears again from ⚙ or from Settings, keeping either side.
- **B is for listening only.** While A/B is on B (Flat), editing pauses with a note to switch back to A. Before, adding or pasting a filter on B could replace your preset with Flat plus that one filter.
- **Copying from Peace or a config file with separate left and right sections keeps them separate**, instead of merging both sides into one chain.
- Renaming a preset keeps its undo history.
- Pasting filters adds a step to the undo history again.

## 0.6.1

- **Hold to compare.** In the A/B hotkey's settings (⚙ next to Flip A/B in Settings › Hotkeys), turn on "Hold to compare": hold the hotkey to switch, let go to switch back. Choose "hold for flat" to hear your EQ and hold for flat, or "hold for your EQ" to hear flat and hold for your EQ. Blind tests still switch on each press.

## 0.6.0

- **Settings for each output device.** Click ⚙ next to a device in the device menu. Pick a preset there and it loads whenever that device becomes your Windows default, however you switched. Volume rises still fade in, and it never switches during a blind test. The same panel shows whether Equalizer APO is on that device and opens APO's device selector. Thanks for the request (#7)!
- **Hotkeys for your devices.** Ctrl+Shift+T moves to the next device that has Equalizer APO. Give any device its own hotkey from its settings (the next free Ctrl+Shift+1…0, or any combination you like). A small popup names the device and its preset; it never takes focus from a game. Thanks for the request (#10)!
- **Fletcher follows device switches made anywhere.** Switching in Windows' sound settings, the taskbar or another app now updates the device menu at once, and a song playing in Clip Studio moves to the new device right where it was (#8).
- **Ctrl+Shift+O opens Fletcher** from the tray or from behind other windows.
- **Every hotkey has its own settings** (⚙): reset, and whether it shows the popup. The A/B popup is off by default and never appears during a blind test.
- **Exclusive playback works on more outputs.** Some outputs (S/PDIF, HDMI, some Bluetooth headphones) claim to support a format and then refuse it. Fletcher now finds one they actually play. If there is none, Clip Studio plays through the normal path instead of failing, and the test tone says why.
- The "APO is now registered" note can be dismissed.

## 0.5.0

- **Change the A/B hotkey.** In Settings › Hotkeys, click the hotkey and press any combination with Ctrl, Alt or Win, switch it off (your combination is kept for later), or reset it. Handy when Ctrl+Shift+A collides with a game. Thanks for the report (#5)!
- **The EQ graph starts at 10 Hz**, the lowest frequency a filter can use, so everything a filter does is visible.
- **Shorter Settings.** Every explanation is now one line, or gone.

## 0.4.0

- **A filter editor that shows you what each filter does.** Click ⤢ on any filter for a larger editor: its own curve drawn over your whole EQ, sliders and exact values for frequency, gain and Q, and every filter type as a small preview drawn from your current settings. Hover a type to read what it does, click to switch. Pop it out into its own window: it follows whichever filter you select, and Ctrl+Z works there too.
- **Filter descriptions are plain facts now**, and some were corrected: Fletcher always writes a Q, and Equalizer APO uses it on every filter type.
- **Dragging is more precise.** The last bit of a quick drag could be lost and the value would jump back slightly; it now lands exactly where you let go.
- **Turning a filter on or off no longer adds an undo step**, and undo keeps the filter you're working on selected.
- Tooltips no longer get stuck on screen after clicking a menu item.
- The bar above the graph no longer wraps onto two lines in a narrow window.
- The custom scale button keeps its width, so the scale buttons don't shift while you drag.

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
