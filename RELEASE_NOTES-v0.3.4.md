# Bea v0.3.4

## What's New

- **Chat answers now render as real Markdown** — headings, bullet and numbered lists, task lists, **bold**/*italic*, blockquotes, links, strikethrough, and fenced code blocks. Previously every answer was flat pre-wrapped text, so `**bold**` and `| a | b |` showed up literally.
- **GitHub-Flavored Markdown tables with a per-table toolbar** — every table Bea produces now has **Copy** (TSV, pastes straight into Excel/Sheets), **Copy as Markdown**, **Download CSV**, and **Download XLSX**. Wide tables scroll horizontally with sticky headers instead of stretching the bubble.
- **Export the whole conversation** — the chat command row has an export menu with **Markdown (.md)** and **Plain text (.txt)**. Both are generated from the stored database record, so a conversation exported after a restart is complete.
- **Copy any message** — one click copies an answer's original Markdown source, so it pastes back into an editor intact rather than as flattened text.

## Chat context now follows the model

- **Compaction is dynamic, not a fixed budget.** The old behaviour used one hardcoded budget, which is wrong in both directions: it overflows a 4k local model and needlessly throws away a 1M one. Every chat question now resolves the answering model's real context window and divides it — transcript 66%, meeting ledger 12%, everything else (system prompt, notes, chat history, cross-meeting memory, slide OCR, attached images) 12%, and 10% held back for the answer.
- **Large windows are genuinely used.** 256k, 500k, 1M and 1.3M models each get their own budget, so a 1.3M model can carry a whole long meeting verbatim where a 32k model gets a compacted one. The ceiling is 4M tokens, so a 2M window is not silently trimmed to 400k.
- **The window comes from the provider.** OpenRouter publishes a `context_length` per model and it is recorded the moment you pick the model. Without a catalog value, a built-in table of well-known families (Gemini, GPT, Claude, Llama, Qwen, DeepSeek, Mistral, Grok, …) is used; unknown models fall back to a conservative 32k.
- **Local servers can be told.** Ollama, LM Studio and llama.cpp list model ids but not the context they were launched with, so Bea assumes a small window rather than overflowing it. Settings now has a **Context window** field for the selected model — set 32768 there and chat keeps far more of a long meeting immediately. Clearing the field deletes the stored value, so the model goes back to automatic detection.
- **Switching models transitions in both directions.** The budget is recomputed per question from the *effective* model (the meeting's override, not just the Settings default), so switching to a big model expands the context on the very next question and switching back compacts again, deterministically and with no stored state to migrate. The catalog window is written before the switch is confirmed, so a question sent immediately after never lands on the previous model's budget. The same model also decides whether images go to the model or are read locally with Tesseract OCR.
- **The whole window is accounted for.** The answer reserve and the vision-frame allowance scale with the model too: a 4k window is sent no frames at all, a 32k window gets two, a 128k window gets the full set of eight. Frames or images beyond the allowance are still read locally with OCR, so nothing attached is silently lost.
- **The prompt, the question and the answer all share the budget.** Your question is charged to the window before anything else, because it is the one block that cannot be shortened without changing what was asked — a long question costs OCR detail and notes first, and only then transcript. Slide OCR, cross-meeting memory and the notes/chat replay are each bounded to what is left of the overhead allowance, and the elision marker is paid for out of the transcript's own share rather than added on top of it. What survives is always inside the window: the assistant reports a clear message instead of sending a request the model would reject, and the only case that can do that is a question that is larger than the model's window on its own.

## Improvements

- **Retry** — a *Retry* action on the latest Bea answer re-sends the paired question and appends the new answer underneath. The previous answer is kept, never deleted.
- **Timestamps** — every bubble shows when it was asked, including conversations reloaded from a previous session.
- **Scroll behaves** — reading back through a long answer no longer yanks you to the bottom. A "jump to latest" button appears when you scroll away.
- **Multiline composer** — the chat input auto-grows, Enter sends, Shift+Enter inserts a newline. Markdown answers are frequently multi-paragraph, so composing across lines matters now.
- **Bea is asked for tables** — the system prompt now asks for GFM formatting and a Markdown table whenever an answer compares three or more things across two or more fields, and the answer cap was raised from 1,500 to 2,500 tokens so a table plus prose is not truncated mid-answer.

## Under the Hood

- **Updater signing key rotated back to `bea.key`.** 0.3.2 introduced a second keypair, and 0.3.4 returns to the original. Installs of 0.3.1–0.3.3 carry the old key and therefore cannot auto-update into this release — download the installer once manually, and every future release updates in-app as normal.
- Rendering uses `react-markdown` + `remark-gfm`, which build a React element tree — no `dangerouslySetInnerHTML`, so the app's `script-src 'self'` CSP still holds.
- CSV and XLSX are written in Rust via the `export_table_command` / `export_chat_command` commands, mirroring the existing minutes-export path (extension validated, refuses to overwrite, creates parent directories). WebView2 silently swallows blob downloads, so the desktop path must be a real file write.
- CSV is RFC 4180 quoted with CRLF endings and a UTF-8 BOM, so accented and CJK speaker names survive a double-click in Excel.
- XLSX is hand-rolled OOXML through the already-present `zip` crate — **zero new Rust dependencies** — using inline strings so there is no `sharedStrings.xml` bookkeeping.
- Cells starting with `=`, `+`, `-`, or `@` are prefixed with `'` in both CSV and XLSX, so a model-generated value can never execute as a live Excel formula.
- Release tags are now strict three-component semver. The old `vX.Y` handling would have published `v0.3.4` as `0.3.0` in `latest.json`; the workflow trigger, the manifest generator, and the version→tag mapping were all corrected.
- 15 new frontend tests cover table extraction and TSV/Markdown/CSV serialization; new Rust tests cover payload parsing, both chat export formats, CSV quoting and formula mitigation, and the XLSX package contents.
- Context windows are stored in a new `model_context_windows` table (model id → tokens → where the value came from), created through the existing runtime schema migration so existing workspaces upgrade in place with no data loss.
- Budget tests pin the behaviour at 2k, 4k, 8k, 32k, 128k, 256k, 500k, 1M, 1.3M and 2M windows: shares must never sum past the window, a 1.3M model must keep a whole meeting a 32k model has to compact, and a 32k → 1.3M → 32k model switch must be deterministic and reversible.

## Downloads

- `Bea_0.3.4_x64-setup.exe` — Windows installer (NSIS)
- `Bea_0.3.4_x64-setup.exe.sig` — minisign updater signature
- `Bea_0.3.4_x64_en-US.msi` — Windows MSI package
