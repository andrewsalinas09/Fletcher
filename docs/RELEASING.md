# Releasing Fletcher

How a version gets from `main` to users' machines. The why is in ADR-0015.

## Cutting a release

```powershell
node scripts/bump-version.mjs 0.2.0   # package.json, lockfiles, tauri.conf.json, both Cargo.tomls, CHANGELOG stub
# write the "## 0.2.0" notes in CHANGELOG.md — for listeners, not developers
git commit -am "Release 0.2.0"
git tag v0.2.0
git push origin main v0.2.0
```

The **Release** workflow (`.github/workflows/release.yml`) then:

1. refuses a tag that doesn't match `tauri.conf.json`'s version,
2. refuses an empty CHANGELOG section,
3. runs `cargo test --workspace`,
4. builds and signs the NSIS installer plus its updater signature,
5. creates a **draft** release on this repo with the installer and `latest.json`.

Download the draft's installer and check it. Then press **Publish** on GitHub. Only a published, non-prerelease release becomes "latest", so nothing reaches users before that click. Installed copies check 8 s after launch and daily after that. A waiting update shows as a dot on the SETTINGS tab, and Settings › UPDATES shows the notes and an install button.

To pull a bad release, delete it (or mark it pre-release); "latest" falls back to the previous one. Anyone who already installed it needs a newer fixed version, because the updater never downgrades.

## Keys and secrets

The updater only installs files signed with the private key matching `plugins.updater.pubkey` in `app/src-tauri/tauri.conf.json`. **If that key is lost, installed copies can never update again. If it leaks, someone else can ship code to every user.** It lives outside the repository:

- `~/.tauri/fletcher-updater.key` (the private key, password-protected)
- `~/.tauri/fletcher-updater.password.txt`
- `~/.tauri/fletcher-updater.key.pub` (public; its contents are the `pubkey` value)

Keep a copy of the first two somewhere safe off this machine.

Repository secrets (Settings → Secrets and variables → Actions), already set:

| Secret | Value |
| --- | --- |
| `TAURI_SIGNING_PRIVATE_KEY` | contents of `fletcher-updater.key` |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | contents of `fletcher-updater.password.txt` |

To (re)set them from this machine:

```powershell
$r='andrewsalinas09/Fletcher'
Get-Content -Raw "$HOME\.tauri\fletcher-updater.key" | gh secret set TAURI_SIGNING_PRIVATE_KEY --repo $r
Get-Content -Raw "$HOME\.tauri\fletcher-updater.password.txt" | gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD --repo $r
```

The workflow uses its own `GITHUB_TOKEN` (`permissions: contents: write`) to create the release. It needs no personal token.

## Local release builds

`createUpdaterArtifacts` makes `npm run tauri build` sign at build time, so it needs the key in the environment:

```powershell
$env:TAURI_SIGNING_PRIVATE_KEY = Get-Content -Raw "$HOME\.tauri\fletcher-updater.key"
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = (Get-Content -Raw "$HOME\.tauri\fletcher-updater.password.txt").Trim()
cd app; npm run tauri build
```

`npm run tauri dev` needs neither. Dev builds can check for updates but refuse to install them, since an installer can't replace a binary in `target/`.
