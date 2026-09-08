# Updating Bea

Bea auto-updates from GitHub Releases (`github.com/NitroQ/bea`). The app checks
for updates ~30s after startup and then every 6 hours (see
`src-tauri/src/updater.rs`). Checks are passive — installing always requires
your click on the in-app banner ("Install & restart").

## How it works

1. Pushing tag `vX.Y` (e.g. `v1.0`) triggers `.github/workflows/release.yml`.
2. CI builds the frontend, builds + **signs** the NSIS/MSI bundles with
   `TAURI_SIGNING_PRIVATE_KEY` (GitHub Secret), generates `latest.json` via
   `scripts/latest-json.mjs`, and attaches everything to the release.
3. Installed apps fetch
   `https://github.com/NitroQ/bea/releases/latest/download/latest.json`
   (pinned in `src-tauri/tauri.conf.json`), compare versions, and — after you
   click Install — download the setup exe, **verify its minisign signature
   against the public key embedded in the app**, and run the installer.

## Releasing

```bash
# 1. Bump version in package.json AND src-tauri/tauri.conf.json AND
#    src-tauri/Cargo.toml (keep all three identical, X.Y.Z).
#    Tag vX.Y maps to app version X.Y.0.
# 2. Commit the bump, then:
git tag v1.1
git push origin master v1.1
# 3. Watch Actions → release. When green, the release page has:
#    latest.json, Bea_1.1.0_x64-setup.exe(.sig), Bea_1.1.0_x64_en-US.msi
```

## Versioning rules

- Tags are **two digits only**: `v1.0`, `v1.1`. Three-component tags
  (`v1.0.1`) are rejected by both the workflow trigger and
  `scripts/latest-json.mjs`. Need a hotfix? Ship `v1.1` (or widen the tag
  regex later).
- Internal versions must stay `X.Y.Z` (Cargo/npm semver) — `X.Y` tags map to
  `X.Y.0`.
- `src/version.test.ts` fails the suite if `package.json` and
  `tauri.conf.json` drift apart.

## Security

- Bundles are minisign-verified; unsigned or tampered downloads are refused.
- The update endpoint is pinned (HTTPS, this repo only).
- The signing private key lives in `~/.tauri/bea.key` (never committed) and in
  the repo's GitHub Secrets (`TAURI_SIGNING_PRIVATE_KEY`,
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` — empty since the key has no password).
- **Back up `~/.tauri/bea.key` somewhere safe.** Losing it means users can't
  verify any future update; you'd have to ship a new pubkey, which requires a
  one-time manual install.

## Rollback

GitHub release assets are immutable. If a release is broken: delete the
release and its tag, fix, and re-tag — `releases/latest/download/latest.json`
then resolves to the previous good release again.
