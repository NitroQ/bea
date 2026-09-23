# Bea v0.3.1

## What's New

- **Transcript is now the default tab** when opening a meeting — the Overview tab has been removed
- **Minutes generation runs in the background** — navigate to the All Meetings page while minutes generate; a status indicator keeps you informed

## Improvements

- The failed-meeting recovery panel (Install FFmpeg, Resume, Start over) now appears at the top of the Transcript tab instead of the removed Overview page
- Library page shows an amber dot and "Generating minutes..." badge on meetings that are generating in the background
- Error toasts now appear with red styling for failed generations on any page
- Removed unused Overview page CSS

## Under the Hood

- Minutes generation ownership moved from the meeting workspace to the app shell, so it survives page navigation
- Single-flight guards prevent duplicate generation requests per meeting
- Custom `bea:minutes-generated` event keeps an open meeting workspace in sync when background generation completes

## Downloads

- `Bea_0.3.1_x64-setup.exe` — Windows installer
- `Bea_0.3.1_x64_en-US.msi` — Windows MSI package
