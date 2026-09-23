# Bea v0.3.3

## What's New

- **Much faster transcription on many-core CPUs** — "Faster transcription" now runs up to 6 parallel decoder sessions, sized to your machine's cores and memory (previously hard-capped at 2). A 3-hour meeting that took ~2 hours now finishes in roughly 20–30 minutes on a 28-core machine.
- **Smarter GPU fallback** — enabling "GPU acceleration" on a build without the DirectML runtime no longer silently drops you to a 2-thread CPU session; Bea now falls back to full CPU turbo parallelism instead.
- **Speaker labels no longer delay transcription** — diarization runs on its own thread while chunks decode. Chunks decoded before the speaker turns arrive stream unlabeled and are relabeled automatically when diarization finishes.

## Improvements

- Chunk slicing for imported media is a single ffmpeg pass (one process for the whole meeting) instead of one process per 28-second chunk — hundreds of sequential spawns removed from long imports.
- Settings screen shows the actual decoder count Bea will use ("Up to N decoders … N cores detected").

## Under the Hood

- `turbo_session_count_with(cores, ram)` scales decoder sessions by cores/4 (clamped 2–6) with a ~1.2 GB per-session RAM budget and 4 GB reserved for the OS and diarization.
- DirectML note: shipping true GPU decoding requires building sherpa-onnx/onnxruntime from source with `-DSHERPA_ONNX_ENABLE_DIRECTML=ON` (no upstream prebuilt ships it); until then the GPU toggle falls back to CPU turbo honestly.

## Downloads

- `Bea_0.3.3_x64-setup.exe` — Windows installer
- `Bea_0.3.3_x64_en-US.msi` — Windows MSI package
