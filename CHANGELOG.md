# Changelog

Each version's section becomes its GitHub release notes and the "what's new" text in Settings › UPDATES. Write for listeners, not developers.

## 0.2.0

- **Fletcher now updates itself.** It checks quietly in the background. When a new version is out, a dot appears on the SETTINGS tab, and Settings › Updates shows what's new and installs it in one click. Updates are cryptographically signed, and they never install during a blind test or a hearing-profile session.
- **"My EQ does nothing" is now explained and fixable.** Windows updates and audio-driver reinstalls can silently remove Equalizer APO from your playback device, leaving every change inaudible. Fletcher now detects this, tells you which device is affected, and one button opens APO's device selector to fix it. The device menu also marks devices without APO.
- **Output trim.** A new Settings › Output control raises or lowers Fletcher's overall level (−20 to +12 dB) without breaking level-matched A/B comparisons. It's for when another tool or a hand-edited config line has left everything too quiet. Fletcher also tells you how much attenuation comes from lines it isn't allowed to touch.
- **If you're on 0.1.0:** install this version by hand from the release page. It's the first one that can update itself, and every update after it arrives inside the app.

## 0.1.0

- First release.
