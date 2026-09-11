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

## Transcription performance modes

- **Thread budget** — one sherpa-onnx session uses half the logical cores
  (`asr_thread_count`, capped at 4) and all transcription work (decodes,
  diarization, ffmpeg children) runs at below-normal OS priority, so
  transcription never starves the user's foreground apps.
- **Faster transcription** (Settings toggle, default off) — builds two
  *independent* sherpa-onnx sessions and decodes chunks in parallel. Gated to
  4+ logical cores and 8+ GB RAM (`turbo_supported_with`); ignored while GPU
  mode is active.
- **GPU acceleration** (Settings toggle, default off) — asks sherpa-onnx for
  the **DirectML** execution provider (any DirectX 12 GPU: NVIDIA/AMD/Intel).
  The toggle is fail-safe: if no DirectML runtime is available the session
  silently falls back to CPU and the Settings label shows the device the last
  run actually used.

### Enabling GPU (DirectML) in builds

Official sherpa-onnx prebuilt libraries (k2-fsa releases, including the static
CPU archive the `sherpa-onnx` Rust crate downloads) are **CPU-only** — there is
no official DirectML/CUDA archive. To make the GPU toggle effective:

1. Build sherpa-onnx from source with the DirectML execution provider, or
   obtain a compatible DirectML shared build of the **same** sherpa-onnx
   version pinned in `src-tauri/Cargo.toml` (mixing versions across the C API
   is not safe):
   `cmake -A x64 -DSHERPA_ONNX_ENABLE_DIRECTML=ON -DBUILD_SHARED_LIBS=ON ...`
   The result ships `sherpa-onnx-c-api.dll`, `onnxruntime.dll` (~40 MB) and
   `DirectML.dll` (~1 MB).
2. Switch `src-tauri/Cargo.toml` to `sherpa-onnx = { version = "…", features = ["shared"] }`
   and point `SHERPA_ONNX_LIB_DIR` at the extracted `lib/` directory before
   building — the sys crate copies any DLLs next to the binary automatically
   (`copy_windows_runtime_dlls`).
3. Verify with `cargo run --release --example transcribe_probe_whisper -- <model_dir> <wav>`:
   the Settings GPU label should then read `On · GPU` after a run (it reports
   the device of the last built session).

Until those libraries are bundled, the toggle exists but every run reports
`Active: cpu` — by design, never a crash or a broken transcript.

## Memory profile

- **Whisper Turbo** ("Bea Standard · Whisper") is the heavyweight: its ONNX
  session needs roughly the model size in RAM (~1.6 GB+, more if only fp16
  files are installed). **Qwen ASR 0.6B INT8** uses roughly half. "Faster
  transcription" (turbo) doubles the model RAM while enabled; **GPU mode**
  shifts the model to GPU memory instead of system RAM.
- RAM is ordered so the speaker-diarization models are loaded and dropped
  **before** the ASR session is built — the two never coexist at peak — and the
  process working set is trimmed right after each run (`trim_process_memory`)
  so freed model pages return to the OS immediately.
- The engine already prefers the smallest (int8) model file present in the
  package (`prefixed_model_file` smallest-match). When upstream publishes an
  int8-only Whisper Turbo package, adding its manifest halves the footprint
  with no code change.
- The Settings → Transcription "Memory" row shows bea.exe plus helper
  processes (ffmpeg/ffprobe/tesseract/WebView runtime) live, for verification.
