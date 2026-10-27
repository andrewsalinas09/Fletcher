# ADR-0015: Self-update from this repo's signed GitHub releases, published by hand

- **Status:** Accepted
- **Date:** 2026-10-26
- **Source:** User directive this session: integrate in-app updates, one repo, keys set up with `gh`

## Context
v0.1.0 shipped as a manually built installer attached to a GitHub release. Without an updater, every fix (TB-30, TB-32) reaches a user only if they notice it and reinstall by hand. Fletcher also sits in a running audio path, so a fix that never arrives is a damaged setup that stays damaged.

## Decision
Fletcher updates itself with Tauri's updater plugin from `releases/latest/download/latest.json` on **this** repository. Pushing a `v*` tag runs `.github/workflows/release.yml`, which builds a signed NSIS installer and creates a **draft** release; a human presses Publish. The check/install commands live in Rust (`updates.rs`), and installing is refused during a blind trial or hearing-profile session.

## Why
- **One repo.** The source is public, so a separate releases-only repo would add a cross-repo token and nothing else. The workflow's own `GITHUB_TOKEN` (contents: write) is enough.
- **Signed, not just HTTPS.** The updater rejects any file not signed by the private key matching `plugins.updater.pubkey`. A compromised release page alone can't push code to users. Key: `~/.tauri/fletcher-updater.key` + password file, both in repo Actions secrets, **never** in git.
- **Draft until Published.** A draft is never `latest`, so a bad build can't reach users by accident. Tested by hand, then one click.
- **Guards in Rust, not the webview.** Installing closes the app. Losing an in-progress blind session or hearing-profile run is a silent data loss (TB-33), so the refusal must not be skippable from the UI. Audio is faded and the device released before the installer runs, never cut mid-buffer.
- **NSIS only, per-user, passive.** One installer type keeps `latest.json` unambiguous; per-user installs need no admin prompt to update. The MSI was dropped. *Rejected:* keeping MSI and NSIS together, since two artifacts per platform make which one the updater picks a question.
- **Notes from CHANGELOG.md.** `scripts/release-notes.mjs` fails the release if the version's section is empty, so "what's new" in Settings is never blank by accident.

## Consequences
- **Losing the private key means installed copies can never update again; leaking it lets someone ship code to every user.** Back up both files outside the machine.
- Local `npm run tauri build` now needs `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` in the environment (`createUpdaterArtifacts`); `tauri dev` doesn't.
- v0.1.0 has no updater: its users must install the first updater-capable release by hand. Every release after that arrives in-app.
- The updater never downgrades. A bad published release is pulled by deleting it (or marking it pre-release), and fixed by shipping a newer version.
