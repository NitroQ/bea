# Bea v0.3.2

## What's New

- **Speaker names persist through interrupted transcriptions** — if Bea closes or crashes mid-transcription, already-completed chunks keep their speaker labels; resuming relabels the rest automatically.
- **Live elapsed-time counter** in the meeting header shows how long the current transcription run has been active (survives tab switches).
- **Detected speakers appear in the Speaker Editor** — any diarization indices found in the transcript are listed automatically so you only name, don't discover.

## Improvements

- Resume transcription now offered on `failed`, `processing`, and `draft` meetings with existing chunks.
- Transcription progress (completed/total chunks) shown alongside elapsed time in the status bar.
- Silence duration added to the transcript toolbar summary.
- New signing key for the Tauri updater — v0.3.1 installs auto-update to this release.

## Under the Hood

- Chunk-level speaker labels persisted immediately on write; full-meeting relabeling pass on resume.
- Start timestamp owned by the App shell, passed to MeetingWorkspace (no local timer reset on navigation).
- SpeakerEditor receives `detectedIndices` from the workspace; unnamed detected speakers shown first.
- SHA-256 checksum manifest (`SHA256SUMS-v0.3.2.txt`) produced alongside installers.

## Downloads

- `Bea_0.3.2_x64-setup.exe` — Windows installer
- `Bea_0.3.2_x64_en-US.msi` — Windows MSI package
- Both signed with the new Tauri updater keypair (`.sig` files included)