// Hides the console window Windows would otherwise allocate for the app on
// launch. Must stay above any `use` statements; only affects release builds
// so `cargo run` keeps showing logs in development.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use bea_core::{
    call_provider, create_job, create_meeting_with_engine, delete_meeting, estimate_tokens,
    export_minutes, extract_ledger_events, generate_minutes, get_app_setting, import_media,
    inspect_asr_model_package, inspect_runtime, install_qwen_model_package,
    install_sherpa_model_package, list_audio_input_devices, list_completed_recording_chunks,
    list_meetings, list_transcript, load_asr_engine, load_minutes, load_provider, open_database,
    persist_completed_audio_chunk, record_usage, register_model, save_ledger_events, save_minutes,
    save_provider, search_transcript, set_app_setting, transcribe_chunks_with_progress,
    transcribe_imported_media_with_progress, update_meeting_title, waveform_peaks, AudioChunkInput,
    CompletedAudioChunk, ContextMode, ExportFormat, FfmpegPipeline, LlmRequest, MediaKind,
    OcrEngine,
    MediaSource, Meeting, Minutes, ModelInstallProgress, ModelManifest, ProviderConfig,
    RecorderConfig, RuntimeAvailability, SegmentedWavRecorder, TranscriptLanguage,
    TranscriptSegment, UsageRecord,
};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use keyring::Entry;
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Manager, State};
use uuid::Uuid;

struct AppState {
    database_path: PathBuf,
    recorders: Mutex<HashMap<String, Sender<RecorderCommand>>>,
    /// Serializes speaker-model downloads: two concurrent transcriptions must
    /// not race through the download/rename staging paths. Tokio mutex because
    /// downloads await while holding it.
    speaker_models_lock: tauri::async_runtime::Mutex<()>,
    /// Meeting ids whose in-flight minutes generation the user cancelled
    /// (wizard "Close"). Checked at the generation checkpoints so a cancelled
    /// run never overwrites saved minutes.
    cancelled_minutes: Mutex<std::collections::HashSet<String>>,
}

enum RecorderCommand {
    Pause(mpsc::SyncSender<Result<(), String>>),
    Resume(mpsc::SyncSender<Result<(), String>>),
    Stop(mpsc::SyncSender<Result<Vec<CompletedAudioChunk>, String>>),
    /// Meters the live input level without touching the recording.
    Level(mpsc::SyncSender<f32>),
}

const MEETING_SECRETARY_SYSTEM_PROMPT: &str = r#"You are an expert meeting secretary. From the transcript events below, produce strict JSON meeting minutes in English.
Rules:
- summary: a 2-4 sentence executive summary of the whole meeting.
- decisions / action_items / unresolved: exactly one entry per distinct point; merge duplicates. Each entry's summary must be a self-contained, third-person, present-tense statement of the outcome or task (e.g. "The passing grade is set to 2.0 effective AY 2026-2027", "Revise the 2-day loss list before Friday") — never a question, never a verbatim or near-verbatim transcript line, and never in the speaker's voice. Include the owner and deadline in the summary when the transcript names them.
- agenda: the meeting's topics in the order they were discussed, each as a short noun-phrase heading (e.g. "Q3 budget review"); attach start_seconds/end_seconds from the transcript when the topic's discussion span is identifiable. Infer topics from how the conversation shifts even when no written agenda exists — every meeting that discussed distinct subjects has an agenda. ALWAYS return at least 1 agenda item when the transcript contains any discussion; the empty array is reserved for transcripts that contain no discussion at all.
- title: a short, specific meeting title derived from the actual content (e.g. "Faculty Council — BPEP Policy Orientation"). Use the meeting's main subject, not a generic label like "Meeting Minutes". When a PREVIOUS AGENDA or existing title context is provided, prefer refining it over replacing it.
- Every item's summary must be ONE concise, capitalized, grammatical headline sentence in English (example: "Admission limits remain at the discretion of the Dean"). NEVER copy raw transcript speech as a summary.
- Every item's evidence: list the EXACT original quotes with the start_seconds/end_seconds taken from the matching input event. Do not paraphrase quotes or invent timestamps.
- Every evidence item's title: one short clause stating WHAT the quote proves (e.g. "Confirms the 2.0 removal-exam passing grade", "Shows Maria owns the CHED submission"). This is the label readers see first.
- Exclude procedural noise (motions to approve past minutes, roll call, greetings, filler) from action items and decisions.
- If a Participants legend or custom format is provided in the system prompt, use real participant names instead of "Speaker N" and follow the custom format's structure while still returning the same JSON schema.
- visual_observations: when visual context (frame images or OCR text of video frames) is provided, add one short observation string per notable thing seen on the frames (e.g. "Slide at 320s shows the Q3 budget table"). Omit the array or return [] when no visual context is provided."#;

/// JSON schema sent with minutes requests so providers with structured output
/// return parseable minutes. Shared by minutes generation and the
/// `validate_minutes_model_command` preflight, which must exercise the exact
/// same contract the real generation path uses.
const MINUTES_JSON_SCHEMA: &str = r#"{"type":"object","properties":{"title":{"type":"string"},"summary":{"type":"string"},"agenda":{"type":"array","items":{"type":"object","properties":{"heading":{"type":"string"},"start_seconds":{"type":"number"},"end_seconds":{"type":"number"}},"required":["heading"]}},"visual_observations":{"type":"array","items":{"type":"string"}},"decisions":{"type":"array","items":{"type":"object","properties":{"kind":{"type":"string"},"summary":{"type":"string"},"confidence":{"type":"number"},"evidence":{"type":"array","items":{"type":"object","properties":{"title":{"type":"string"},"start_seconds":{"type":"number"},"end_seconds":{"type":"number"},"quote":{"type":"string"}},"required":["start_seconds","end_seconds","quote"]}}},"required":["summary","evidence"]}},"action_items":{"type":"array","items":{"type":"object","properties":{"kind":{"type":"string"},"summary":{"type":"string"},"confidence":{"type":"number"},"evidence":{"type":"array","items":{"type":"object","properties":{"title":{"type":"string"},"start_seconds":{"type":"number"},"end_seconds":{"type":"number"},"quote":{"type":"string"}},"required":["start_seconds","end_seconds","quote"]}}},"required":["summary","evidence"]}},"unresolved":{"type":"array","items":{"type":"object","properties":{"kind":{"type":"string"},"summary":{"type":"string"},"confidence":{"type":"number"},"evidence":{"type":"array","items":{"type":"object","properties":{"title":{"type":"string"},"start_seconds":{"type":"number"},"end_seconds":{"type":"number"},"quote":{"type":"string"}},"required":["start_seconds","end_seconds","quote"]}}},"required":["summary","evidence"]}}},"required":["title","summary","decisions","action_items","unresolved"]}"#;

/// Output-token budget for minutes requests. It is a cap, not a target — the
/// model stops when the JSON is complete — so a generous ceiling costs nothing
/// on short meetings while preventing truncation on long ones. Providers that
/// reject a cap above their model's limit get an automatic 8,000-token retry
/// (see `call_provider`).
const MINUTES_MAX_OUTPUT_TOKENS: u32 = 32_000;

fn language_from_code(code: &str) -> TranscriptLanguage {
    match code {
        "en" => TranscriptLanguage::English,
        "fil" => TranscriptLanguage::Filipino,
        "taglish" => TranscriptLanguage::Taglish,
        _ => TranscriptLanguage::Auto,
    }
}

fn command_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

/// Records a failed transcription run durably: the meeting flips to `failed`
/// and the reason is stored on the row (the UI's "Needs attention" panel reads
/// it), plus the latest transcription job keeps the error for diagnostics.
/// Best-effort job update — a missing job row must not hide the meeting state.
fn record_transcription_failure(
    database_path: &std::path::Path,
    meeting_id: &str,
    error: &str,
) -> Result<(), String> {
    let database = open_database(database_path).map_err(command_error)?;
    let _ = database.execute(
        "UPDATE jobs SET state='failed',error=?2 WHERE id=(SELECT id FROM jobs WHERE meeting_id=?1 AND kind='transcription' ORDER BY rowid DESC LIMIT 1)",
        rusqlite::params![meeting_id, error],
    );
    bea_core::mark_meeting_failed(&database, meeting_id, error).map_err(command_error)
}

/// Shared HTTP client with a 200 s timeout. Requests without a timeout used to
/// hang the command forever when a provider stalled; the OpenRouter catalog
/// call keeps its own shorter 30 s timeout.
fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(
            bea_core::PROVIDER_TIMEOUT_SECS,
        ))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Validates a meeting id before it reaches a filesystem path. Rejects empty
/// ids, path separators, parent traversal, and anything outside the safe
/// `[A-Za-z0-9._-]` set so `derived/<id>/` can never escape its root.
fn sanitize_meeting_id(id: &str) -> Result<String, String> {
    if id.is_empty() {
        return Err("meeting id must not be empty".into());
    }
    if id == "." || id == ".." || id.contains('/') || id.contains('\\') || id.contains("..") {
        return Err(format!("unsafe meeting id: {id:?}"));
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(format!("unsafe meeting id: {id:?}"));
    }
    Ok(id.to_string())
}

/// Composes the minutes-failure guidance sentence. Trims any trailing
/// sentence period off the provider error first, so the guidance never
/// renders a double ".." ("…fallback.. Fix the provider…").
fn minutes_failure_message(provider_error: &str) -> String {
    let trimmed = provider_error.trim_end_matches(['.', ' ']);
    format!("AI minutes failed: {trimmed}. Fix the provider in Settings, or disable it to use local heuristic minutes.")
}

/// Resolves the playback source path against the meeting's registered media
/// sources so only imported/recorded media can be transcoded.
fn verify_media_source(database: &rusqlite::Connection, meeting_id: &str, source: &std::path::Path) -> Result<(), String> {
    let mut statement = database
        .prepare("SELECT path FROM media_sources WHERE meeting_id=?1")
        .map_err(command_error)?;
    let rows = statement
        .query_map(rusqlite::params![meeting_id], |row| {
            row.get::<_, String>(0)
        })
        .map_err(command_error)?;
    let source_text = source.to_string_lossy();
    let source_lower = source_text.to_ascii_lowercase();
    for registered in rows {
        let registered = registered.map_err(command_error)?;
        let registered_lower = registered.to_ascii_lowercase();
        if registered_lower == source_lower {
            return Ok(());
        }
        // Windows paths may differ in separator style; compare canonical forms
        // where possible and fall back to separator-normalized text.
        if let (Ok(canon_reg), Ok(canon_src)) = (
            std::fs::canonicalize(&registered),
            std::fs::canonicalize(source),
        ) {
            if canon_reg == canon_src {
                return Ok(());
            }
        }
        let normalize = |value: &str| value.replace('\\', "/");
        if normalize(&registered_lower) == normalize(&source_lower) {
            return Ok(());
        }
    }
    Err("media path is not registered for this meeting".into())
}

fn list_context_events_payload(
    database: &rusqlite::Connection,
    meeting_id: &str,
) -> Result<String, String> {
    // Chat Q&A persisted with kind 'chat' must feed future minutes too.
    let mut statement = database
        .prepare("SELECT kind,payload FROM context_events WHERE meeting_id=?1 AND kind IN ('clarify','context','chat') ORDER BY created_at, rowid")
        .map_err(command_error)?;
    let rows = statement
        .query_map(rusqlite::params![meeting_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(command_error)?;
    let collected = rows.collect::<Result<Vec<_>, _>>().map_err(command_error)?;
    if collected.is_empty() {
        return Ok("(none)".into());
    }
    Ok(collected
        .iter()
        .map(|(kind, payload)| format!("- [{kind}] {payload}"))
        .collect::<Vec<_>>()
        .join("\n"))
}

#[cfg(test)]
mod context_payload_tests {
    use super::list_context_events_payload;

    /// Chat Q&A (kind 'chat') must reach the minutes prompt alongside
    /// 'clarify' and 'context' events.
    #[test]
    fn chat_events_are_included_in_context_payload() {
        let database = rusqlite::Connection::open_in_memory().unwrap();
        database
            .execute_batch(
                "CREATE TABLE context_events (id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL, kind TEXT NOT NULL, payload TEXT NOT NULL, confidence REAL NOT NULL, created_at TEXT NOT NULL);",
            )
            .unwrap();
        for (kind, payload) in [
            ("clarify", "What date?"),
            ("context", "Budget frozen"),
            ("chat", "Q: Who approved?\nA: Maria"),
            ("ignored", "noise"),
        ] {
            database
                .execute(
                    "INSERT INTO context_events(id,meeting_id,kind,payload,confidence,created_at) VALUES (?1,'m1',?2,?3,1.0,'2026-01-01')",
                    rusqlite::params![uuid::Uuid::new_v4().to_string(), kind, payload],
                )
                .unwrap();
        }
        let payload = list_context_events_payload(&database, "m1").unwrap();
        assert!(payload.contains("[clarify] What date?"));
        assert!(payload.contains("[context] Budget frozen"));
        assert!(payload.contains("[chat] Q: Who approved?"));
        assert!(!payload.contains("noise"));
    }
}

/// Builds the cross-meeting memory block injected into the chat system
/// prompt: an index of all other meetings (title + date) plus FTS-matched
/// transcript passages relevant to the question. Empty when there are no
/// other meetings, so the prompt stays unchanged.
fn build_memory_block(
    database: &rusqlite::Connection,
    exclude_meeting_id: &str,
    question: &str,
) -> Result<String, String> {
    let mut statement = database
        .prepare("SELECT title, created_at FROM meetings WHERE id != ?1 ORDER BY created_at DESC LIMIT 15")
        .map_err(command_error)?;
    let meetings = statement
        .query_map(rusqlite::params![exclude_meeting_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(command_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(command_error)?;
    if meetings.is_empty() {
        return Ok(String::new());
    }
    let mut block = format!(
        "\n\n=== MEMORY: OTHER MEETINGS ({} recorded) ===\nThe user may reference earlier meetings. This index lists them:\n",
        meetings.len()
    );
    for (title, created_at) in &meetings {
        let date = created_at.get(..10).unwrap_or(created_at);
        block.push_str(&format!("- {title} (recorded {date})\n"));
    }
    let matches = bea_core::search_memory(database, exclude_meeting_id, question, 12)
        .map_err(command_error)?;
    if !matches.is_empty() {
        block.push_str("\nRelevant passages found in other meetings' transcripts:\n");
        for entry in matches {
            if block.len() > 4000 {
                break;
            }
            block.push_str(&format!(
                "[{}] [{:02}:{:02}] {}\n",
                entry.meeting_title,
                entry.timestamp_seconds / 60,
                entry.timestamp_seconds % 60,
                entry.text.trim()
            ));
        }
    }
    block.push_str("If the question references an earlier meeting, use this memory. Otherwise ignore it.");
    Ok(block)
}

#[cfg(test)]
mod memory_block_tests {
    use super::build_memory_block;

    fn memory_db() -> rusqlite::Connection {
        let database = rusqlite::Connection::open_in_memory().unwrap();
        database
            .execute_batch(
                "CREATE TABLE meetings (id TEXT PRIMARY KEY, title TEXT NOT NULL, status TEXT NOT NULL, created_at TEXT NOT NULL, duration_seconds INTEGER NOT NULL DEFAULT 0, language TEXT NOT NULL DEFAULT 'auto', asr_engine_id TEXT NOT NULL DEFAULT 'qwen-standard');
                 CREATE TABLE transcript_segments (id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL, start_seconds INTEGER NOT NULL, end_seconds INTEGER NOT NULL, text TEXT NOT NULL, language_detected TEXT, language_confidence REAL, speaker INTEGER);
                 CREATE VIRTUAL TABLE transcript_fts USING fts5(meeting_id UNINDEXED, segment_id UNINDEXED, text);",
            )
            .unwrap();
        database
    }

    fn insert_meeting(database: &rusqlite::Connection, id: &str, title: &str) {
        database
            .execute(
                "INSERT INTO meetings(id,title,status,created_at) VALUES (?1,?2,'ready','2026-03-05T10:00:00Z')",
                rusqlite::params![id, title],
            )
            .unwrap();
    }

    fn insert_segment(database: &rusqlite::Connection, id: &str, meeting_id: &str, text: &str) {
        database
            .execute(
                "INSERT INTO transcript_segments(id,meeting_id,start_seconds,end_seconds,text) VALUES (?1,?2,0,10,?3)",
                rusqlite::params![id, meeting_id, text],
            )
            .unwrap();
        database
            .execute(
                "INSERT INTO transcript_fts(meeting_id,segment_id,text) VALUES (?1,?2,?3)",
                rusqlite::params![meeting_id, id, text],
            )
            .unwrap();
    }

    #[test]
    fn empty_when_no_other_meetings() {
        let database = memory_db();
        insert_meeting(&database, "m1", "Only meeting");
        assert!(
            build_memory_block(&database, "m1", "anything")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn lists_meetings_and_passages() {
        let database = memory_db();
        insert_meeting(&database, "m1", "Current meeting");
        insert_meeting(&database, "m2", "Budget review");
        insert_segment(&database, "s1", "m2", "budget was approved last week");
        let block = build_memory_block(&database, "m1", "budget approved").unwrap();
        assert!(block.contains("=== MEMORY: OTHER MEETINGS (1 recorded) ==="));
        assert!(block.contains("- Budget review (recorded 2026-03-05)"));
        assert!(block.contains("[Budget review] [00:00] budget was approved last week"));
        assert!(block.contains("Otherwise ignore it."));
    }

    #[test]
    fn index_only_when_no_passage_matches() {
        let database = memory_db();
        insert_meeting(&database, "m1", "Current meeting");
        insert_meeting(&database, "m2", "Kickoff");
        let block = build_memory_block(&database, "m1", "something unrelated xyzzy").unwrap();
        assert!(block.contains("- Kickoff (recorded 2026-03-05)"));
        assert!(!block.contains("Relevant passages"));
    }
}

#[cfg(test)]
mod chat_prompt_tests {
    use super::{build_memory_block, chat_system_prompt};

    #[test]
    fn chat_system_prompt_carries_the_memory_block() {
        let prompt = chat_system_prompt(
            "CTXPROMPT",
            "NOTESBLOCK",
            "LEDGERBLOCK",
            "TRANSCRIPTBODY",
            "",
            "\n\n=== MEMORY: OTHER MEETINGS (1 recorded) ===\n- Kickoff (recorded 2026-03-05)\n",
        );
        assert!(prompt.starts_with("You are Bea,"));
        assert!(prompt.contains("CTXPROMPT"));
        assert!(prompt.contains("=== MEETING NOTES (user-added clarifications/context) ===\nNOTESBLOCK"));
        assert!(prompt.contains("=== MEETING LEDGER ===\nLEDGERBLOCK"));
        assert!(prompt.contains("=== FULL TRANSCRIPT ===\nTRANSCRIPTBODY"));
        // The cross-meeting memory must ride in the same prompt.
        assert!(prompt.contains("=== MEMORY: OTHER MEETINGS (1 recorded) ==="));
    }

    #[test]
    fn chat_system_prompt_stays_unchanged_without_memory() {
        let prompt = chat_system_prompt("CTX", "NOTES", "LEDGER", "TRANSCRIPT", "", "");
        assert!(!prompt.contains("MEMORY"));
        assert!(prompt.ends_with("TRANSCRIPT"));
    }

    #[test]
    fn chat_memory_block_flows_from_the_database_into_the_prompt() {
        // Mirrors the chat_command wiring: build_memory_block(db, id, question)
        // feeds chat_system_prompt so FTS passages from other meetings actually
        // reach the model.
        let database = rusqlite::Connection::open_in_memory().unwrap();
        database
            .execute_batch(
                "CREATE TABLE meetings (id TEXT PRIMARY KEY, title TEXT NOT NULL, status TEXT NOT NULL, created_at TEXT NOT NULL, duration_seconds INTEGER NOT NULL DEFAULT 0, language TEXT NOT NULL DEFAULT 'auto', asr_engine_id TEXT NOT NULL DEFAULT 'qwen-standard');
                 CREATE TABLE transcript_segments (id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL, start_seconds INTEGER NOT NULL, end_seconds INTEGER NOT NULL, text TEXT NOT NULL, language_detected TEXT, language_confidence REAL, speaker INTEGER);
                 CREATE VIRTUAL TABLE transcript_fts USING fts5(meeting_id UNINDEXED, segment_id UNINDEXED, text);",
            )
            .unwrap();
        database
            .execute(
                "INSERT INTO meetings(id,title,status,created_at) VALUES ('m1','Current','ready','2026-03-05T10:00:00Z')",
                [],
            )
            .unwrap();
        database
            .execute(
                "INSERT INTO meetings(id,title,status,created_at) VALUES ('m2','Budget review','ready','2026-03-04T10:00:00Z')",
                [],
            )
            .unwrap();
        database
            .execute(
                "INSERT INTO transcript_segments(id,meeting_id,start_seconds,end_seconds,text) VALUES ('s1','m2',0,10,'budget was approved last week')",
                [],
            )
            .unwrap();
        database
            .execute(
                "INSERT INTO transcript_fts(meeting_id,segment_id,text) VALUES ('m2','s1','budget was approved last week')",
                [],
            )
            .unwrap();
        let memory = build_memory_block(&database, "m1", "who approved the budget?").unwrap();
        let prompt = chat_system_prompt("CTX", "NOTES", "LEDGER", "TRANSCRIPT", "", &memory);
        assert!(prompt.contains("[Budget review] [00:00] budget was approved last week"));
    }
}

fn locate_tesseract(root: &std::path::Path) -> PathBuf {
    let mut candidates = Vec::new();
    if let Ok(executable) = std::env::current_exe() {
        if let Some(parent) = executable.parent() {
            // Development builds place resource directories beside the binary;
            // installed Tauri bundles may place them under resources/.
            candidates.push(
                parent
                    .join("binaries")
                    .join("tesseract")
                    .join("tesseract.exe"),
            );
            candidates.push(
                parent
                    .join("resources")
                    .join("binaries")
                    .join("tesseract")
                    .join("tesseract.exe"),
            );
            candidates.push(
                parent
                    .join("resources")
                    .join("tesseract")
                    .join("tesseract.exe"),
            );
        }
    }
    candidates.push(root.join("bin").join("tesseract").join("tesseract.exe"));
    if let Ok(program_files) = std::env::var("ProgramFiles") {
        candidates.push(
            PathBuf::from(program_files)
                .join("Tesseract-OCR")
                .join("tesseract.exe"),
        );
    }
    if let Ok(program_files_x86) = std::env::var("ProgramFiles(x86)") {
        candidates.push(
            PathBuf::from(program_files_x86)
                .join("Tesseract-OCR")
                .join("tesseract.exe"),
        );
    }
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .unwrap_or_else(|| root.join("bin").join("tesseract").join("tesseract.exe"))
}

fn copy_directory(source: &std::path::Path, destination: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(destination).map_err(command_error)?;
    for entry in std::fs::read_dir(source).map_err(command_error)? {
        let entry = entry.map_err(command_error)?;
        let from = entry.path();
        let to = destination.join(entry.file_name());
        if from.is_dir() {
            copy_directory(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).map_err(command_error)?;
        }
    }
    Ok(())
}

fn bundled_binary_candidates(name: &str) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(executable) = std::env::current_exe() {
        if let Some(parent) = executable.parent() {
            candidates.push(parent.join(name));
            candidates.push(parent.join("resources").join(name));
            candidates.push(parent.join("resources").join("binaries").join(name));
        }
    }
    let source_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    candidates.push(source_root.join("binaries").join(name));
    if name == "ffmpeg.exe" {
        candidates.push(
            source_root
                .join("binaries")
                .join("ffmpeg-x86_64-pc-windows-msvc.exe"),
        );
    }
    if name == "ffprobe.exe" {
        candidates.push(
            source_root
                .join("binaries")
                .join("ffprobe-x86_64-pc-windows-msvc.exe"),
        );
    }
    candidates
}

/// Locate a usable FFmpeg/FFprobe pair as a unit. Tauri development builds place
/// sidecars beside `bea.exe`; packaged builds may place them under resources, and
/// repaired installations use the app-data `bin` directory. A lone ffmpeg file is
/// not enough because media probing requires ffprobe too.
fn locate_ffmpeg(root: &std::path::Path) -> PathBuf {
    let mut directories = Vec::new();
    if let Ok(executable) = std::env::current_exe() {
        if let Some(parent) = executable.parent() {
            directories.push(parent.to_path_buf());
            directories.push(parent.join("resources"));
            directories.push(parent.join("resources").join("binaries"));
        }
    }
    directories.push(root.join("bin"));
    directories.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("binaries"));
    let ffmpeg_names = ["ffmpeg.exe", "ffmpeg-x86_64-pc-windows-msvc.exe"];
    let ffprobe_names = ["ffprobe.exe", "ffprobe-x86_64-pc-windows-msvc.exe"];
    for directory in &directories {
        for ffmpeg_name in ffmpeg_names {
            let ffmpeg = directory.join(ffmpeg_name);
            if !ffmpeg.is_file() {
                continue;
            }
            if ffprobe_names
                .iter()
                .any(|name| directory.join(name).is_file())
            {
                return ffmpeg;
            }
        }
    }
    root.join("bin").join("ffmpeg.exe")
}

#[allow(clippy::too_many_arguments)]
fn build_input_stream(
    device: &cpal::Device,
    supported: &cpal::SupportedStreamConfig,
    recorder: Arc<Mutex<SegmentedWavRecorder>>,
    mixer: Arc<Mutex<bea_core::AudioMixer>>,
    source_frames: Arc<Mutex<HashMap<&'static str, u64>>>,
    source_key: &'static str,
    stream_error: Arc<Mutex<Option<String>>>,
) -> Result<cpal::Stream, String> {
    let config = supported.config();
    // Stream errors (device unplugged, format change, ...) are surfaced
    // through a shared slot instead of being silently dropped; the stop path
    // reports them.
    let error_callback = move |error: cpal::StreamError| {
        if let Ok(mut slot) = stream_error.lock() {
            *slot = Some(error.to_string());
        }
    };
    // Every sample batch is resampled/downmixed into the master format, summed
    // into the shared mixer at the source's own timeline position, and only the
    // settled prefix reaches the recorder — so mic + desktop loopback produce
    // ONE aligned chunk sequence for the ASR pipeline.
    let master_channels = config.channels;
    let master_rate = config.sample_rate.0;
    let build = move |data: &[i16]| -> Option<Vec<i16>> {
        if data.is_empty() {
            return None;
        }
        let channels = master_channels.max(1) as usize;
        let delivered = data.len() / channels;
        let before = source_frames
            .lock()
            .map(|mut slots| {
                let previous = *slots.get(source_key).unwrap_or(&0);
                slots.insert(source_key, previous + delivered as u64);
                previous
            })
            .unwrap_or(0);
        let mut mixer = mixer.lock().ok()?;
        let settled = mixer.push(before, data, master_rate, master_channels);
        (!settled.is_empty()).then_some(settled)
    };
    match supported.sample_format() {
        cpal::SampleFormat::F32 => device.build_input_stream(
            &config,
            move |data: &[f32], _| {
                let samples: Vec<i16> = data
                    .iter()
                    .map(|sample| (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
                    .collect();
                if let Some(settled) = build(&samples) {
                    if let Ok(mut recorder) = recorder.lock() {
                        let _ = recorder.push_samples(&settled);
                    }
                }
            },
            error_callback,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            &config,
            move |data: &[i16], _| {
                if let Some(settled) = build(data) {
                    if let Ok(mut recorder) = recorder.lock() {
                        let _ = recorder.push_samples(&settled);
                    }
                }
            },
            error_callback,
            None,
        ),
        cpal::SampleFormat::U16 => device.build_input_stream(
            &config,
            move |data: &[u16], _| {
                let samples: Vec<i16> = data
                    .iter()
                    .map(|sample| (*sample as i32 - 32768) as i16)
                    .collect();
                if let Some(settled) = build(&samples) {
                    if let Ok(mut recorder) = recorder.lock() {
                        let _ = recorder.push_samples(&settled);
                    }
                }
            },
            error_callback,
            None,
        ),
        format => return Err(format!("unsupported input sample format: {format:?}")),
    }
    .map_err(|error| error.to_string())
}

fn spawn_recorder(
    root: PathBuf,
    ready: mpsc::SyncSender<Result<(), String>>,
    device_ids: Vec<String>,
) -> Sender<RecorderCommand> {
    let (commands, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| -> Result<(), String> {
            let host = cpal::default_host();
            // One entry per requested source; no ids → the default microphone.
            let sources: Vec<(cpal::Device, &str)> = if device_ids.is_empty() {
                let device = host
                    .default_input_device()
                    .ok_or_else(|| "no default microphone is available".to_string())?;
                vec![(device, "mic")]
            } else {
                device_ids
                    .iter()
                    .map(|id| {
                        bea_core::resolve_audio_device(Some(id))
                            .map_err(command_error)
                            .map(|(device, kind)| (device, kind))
                    })
                    .collect::<Result<Vec<_>, String>>()?
            };
            // Master format: the first source's native configuration. Every
            // source is converted (downmix/resample) into this shape, so the
            // recorder and the downstream ASR pipeline see one consistent
            // timeline.
            let master_config = sources
                .first()
                .and_then(|(device, _)| device.default_input_config().ok())
                .ok_or_else(|| "no usable audio configuration for the selected devices".to_string())?;
            let master_rate = master_config.sample_rate().0;
            let master_channels = master_config.channels();
            let mut recorder = SegmentedWavRecorder::new(
                root,
                RecorderConfig {
                    sample_rate: master_rate,
                    channels: master_channels,
                    // Whisper-family engines only process the first 30 s of an
                    // input; keep recorded chunks under that ceiling.
                    chunk_seconds: 28,
                    max_duration_seconds: 5 * 60 * 60,
                },
            )
            .map_err(command_error)?;
            recorder.start().map_err(command_error)?;
            let recorder = Arc::new(Mutex::new(recorder));
            let stream_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
            let mixer: Arc<Mutex<bea_core::AudioMixer>> = Arc::new(Mutex::new(bea_core::AudioMixer::new(
                master_rate,
                master_channels,
            )));
            let source_frames: Arc<Mutex<HashMap<&'static str, u64>>> =
                Arc::new(Mutex::new(HashMap::new()));
            let mut streams = Vec::new();
            for (device, kind) in &sources {
                let supported = device.default_input_config().map_err(command_error)?;
                let stream = build_input_stream(
                    device,
                    &supported,
                    Arc::clone(&recorder),
                    Arc::clone(&mixer),
                    Arc::clone(&source_frames),
                    if *kind == "output" { "output" } else { "mic" },
                    Arc::clone(&stream_error),
                )?;
                stream.play().map_err(command_error)?;
                streams.push(stream);
            }
            ready
                .send(Ok(()))
                .map_err(|_| "recording startup acknowledgement failed".to_string())?;
            for command in receiver {
                match command {
                    RecorderCommand::Pause(reply) => {
                        let result = recorder
                            .lock()
                            .map_err(|_| "recorder lock poisoned".to_string())
                            .and_then(|mut value| value.pause().map_err(command_error));
                        let _ = reply.send(result);
                    }
                    RecorderCommand::Resume(reply) => {
                        let result = recorder
                            .lock()
                            .map_err(|_| "recorder lock poisoned".to_string())
                            .and_then(|mut value| value.resume().map_err(command_error));
                        let _ = reply.send(result);
                    }
                    RecorderCommand::Level(reply) => {
                        // Peak level of the live master buffer, 0.0-1.0.
                        let level = mixer
                            .lock()
                            .ok()
                            .map(|mixer| mixer.peak_level())
                            .unwrap_or(0.0);
                        let _ = reply.send(level);
                    }
                    RecorderCommand::Stop(reply) => {
                        let result = (|| -> Result<Vec<CompletedAudioChunk>, String> {
                            // Flush the mixer's unsettled tail first so the
                            // last fraction of a second is not lost, then stop
                            // the recorder to finalize the final chunk.
                            let tail = mixer
                                .lock()
                                .map_err(|_| "mixer lock poisoned".to_string())?
                                .drain();
                            let mut recorder = recorder
                                .lock()
                                .map_err(|_| "recorder lock poisoned".to_string())?;
                            recorder.push_samples(&tail).map_err(command_error)?;
                            recorder.stop().map_err(command_error)
                        })();
                        // A stream error mid-recording invalidates the audio:
                        // report it instead of handing back partial data.
                        let stream_error = stream_error
                            .lock()
                            .ok()
                            .and_then(|slot| slot.clone());
                        let result = match (result, stream_error) {
                            (Ok(chunks), Some(error)) if chunks.is_empty() => {
                                Err(format!("audio stream failed: {error}"))
                            }
                            (result, _) => result,
                        };
                        let _ = reply.send(result);
                        break;
                    }
                }
            }
            drop(streams);
            Ok(())
        })();
        if let Err(error) = result {
            let _ = ready.send(Err(error));
        }
    });
    commands
}

#[tauri::command]
fn create_meeting_command(
    state: State<'_, AppState>,
    title: String,
    language: String,
    asr_engine_id: Option<String>,
) -> Result<Meeting, String> {
    let database = open_database(&state.database_path).map_err(command_error)?;
    create_meeting_with_engine(
        &database,
        &title,
        language_from_code(&language),
        asr_engine_id.as_deref().unwrap_or("whisper-compatibility"),
    )
    .map_err(command_error)
}

#[tauri::command]
fn list_meetings_command(state: State<'_, AppState>) -> Result<Vec<Meeting>, String> {
    let database = open_database(&state.database_path).map_err(command_error)?;
    list_meetings(&database).map_err(command_error)
}

#[tauri::command]
fn rename_meeting_command(
    state: State<'_, AppState>,
    meeting_id: String,
    title: String,
) -> Result<(), String> {
    let database = open_database(&state.database_path).map_err(command_error)?;
    update_meeting_title(&database, &meeting_id, &title).map_err(command_error)
}

#[tauri::command]
fn delete_meeting_command(state: State<'_, AppState>, meeting_id: String) -> Result<(), String> {
    let database = open_database(&state.database_path).map_err(command_error)?;
    delete_meeting(&database, &meeting_id).map_err(command_error)
}

/// Wipes every piece of user data: the SQLite database (meetings, transcripts,
/// minutes, chat notes, speakers, usage), generated/derived media, recordings,
/// and the ChatGPT sign-in token file. Downloaded engines and local tools are
/// kept — they are program assets, not user data. The database file is
/// recreated empty on the next command.
#[tauri::command]
fn delete_all_data_command(state: State<'_, AppState>) -> Result<(), String> {
    let root = state
        .database_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    // Best-effort: remove every stored credential we know about so a
    // "delete all" really leaves nothing sensitive behind.
    if let Ok(entry) = Entry::new("bea-provider", CODEX_KEYRING_ID) {
        let _ = entry.delete_credential();
    }
    if let Ok(entry) = Entry::new("bea-provider", "primary") {
        let _ = entry.delete_credential();
    }
    if let Ok(token_path) = codex_token_path() {
        let _ = std::fs::remove_file(token_path);
    }
    for suffix in ["", "-wal", "-shm"] {
        let database_path = PathBuf::from(format!(
            "{}{suffix}",
            state.database_path.to_string_lossy()
        ));
        if database_path.exists() {
            std::fs::remove_file(&database_path)
                .map_err(|error| format!("could not delete the database: {error}"))?;
        }
    }
    for directory in ["derived", "recordings", "media"] {
        let target = root.join(directory);
        if target.exists() {
            std::fs::remove_dir_all(&target)
                .map_err(|error| format!("could not delete {directory}: {error}"))?;
        }
    }
    Ok(())
}

#[tauri::command]
fn update_transcript_segment_command(
    state: State<'_, AppState>,
    segment_id: String,
    text: String,
) -> Result<(), String> {
    let mut database = open_database(&state.database_path).map_err(command_error)?;
    let changed = database
        .execute(
            "UPDATE transcript_segments SET text=?1 WHERE id=?2",
            rusqlite::params![text.trim(), segment_id],
        )
        .map_err(command_error)?;
    if changed == 0 {
        return Err("transcript segment not found".into());
    }
    // Atomic FTS rebuild: a failure between DELETE and INSERT would leave
    // search silently missing this segment.
    let transaction = database.transaction().map_err(command_error)?;
    transaction
        .execute(
            "DELETE FROM transcript_fts WHERE segment_id=?1",
            rusqlite::params![segment_id],
        )
        .map_err(command_error)?;
    transaction
        .execute(
            "INSERT INTO transcript_fts(meeting_id,segment_id,text) SELECT meeting_id,id,text FROM transcript_segments WHERE id=?1",
            rusqlite::params![segment_id],
        )
        .map_err(command_error)?;
    transaction.commit().map_err(command_error)?;
    Ok(())
}

#[tauri::command]
fn delete_transcript_segments_command(
    state: State<'_, AppState>,
    meeting_id: String,
    segment_ids: Vec<String>,
) -> Result<usize, String> {
    let meeting_id = sanitize_meeting_id(&meeting_id)?;
    let database = open_database(&state.database_path).map_err(command_error)?;
    let refs: Vec<&str> = segment_ids.iter().map(String::as_str).collect();
    bea_core::delete_transcript_segments(&database, &meeting_id, &refs).map_err(command_error)
}

fn list_media_command_inner(
    database: &rusqlite::Connection,
    meeting_id: &str,
) -> Result<Vec<MediaSource>, String> {
    let mut statement = database
        .prepare("SELECT id,meeting_id,path,kind,duration_seconds,copied FROM media_sources WHERE meeting_id=?1 ORDER BY rowid")
        .map_err(command_error)?;
    let rows = statement
        .query_map(rusqlite::params![meeting_id], |row| {
            let kind = match row.get::<_, String>(3)?.as_str() {
                "audio" => MediaKind::Audio,
                _ => MediaKind::Video,
            };
            Ok(MediaSource {
                id: row.get(0)?,
                meeting_id: row.get(1)?,
                path: PathBuf::from(row.get::<_, String>(2)?),
                kind,
                duration_seconds: row.get(4)?,
                copied: row.get::<_, i64>(5)? != 0,
            })
        })
        .map_err(command_error)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(command_error)
}

#[tauri::command]
fn list_media_command(
    state: State<'_, AppState>,
    meeting_id: String,
) -> Result<Vec<MediaSource>, String> {
    let database = open_database(&state.database_path).map_err(command_error)?;
    list_media_command_inner(&database, &meeting_id)
}

#[derive(serde::Serialize, Clone)]
struct ExtractedFrame {
    timestamp_seconds: u64,
    path: String,
    ocr_text: Option<String>,
}

/// Grabs one JPEG per requested timestamp from the meeting's video source.
/// Always also runs Tesseract OCR on the frame so the caller can fall back to
/// text when the selected model cannot see images. Frames are cached under
/// `derived/<meeting_id>/frames/` so repeated requests for the same second
/// are idempotent.
#[tauri::command]
fn extract_frames_command(
    state: State<'_, AppState>,
    meeting_id: String,
    timestamps: Vec<u64>,
) -> Result<Vec<ExtractedFrame>, String> {
    let root = state
        .database_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .to_path_buf();
    let database = open_database(&state.database_path).map_err(command_error)?;
    extract_frames_inner(&root, &database, &meeting_id, &timestamps)
}

/// Shared frame-extraction logic used by `extract_frames_command` and by the
/// chat/minutes commands that want visual context. Grabs one JPEG per
/// requested timestamp (cached on disk) and OCRs each frame so callers can
/// fall back to text when the model has no vision.
fn extract_frames_inner(
    root: &std::path::Path,
    database: &rusqlite::Connection,
    meeting_id: &str,
    timestamps: &[u64],
) -> Result<Vec<ExtractedFrame>, String> {
    let meeting_id = sanitize_meeting_id(meeting_id)?;
    let video = list_media_command_inner(database, meeting_id.as_str())?
        .into_iter()
        .find(|media| matches!(media.kind, MediaKind::Video))
        .ok_or_else(|| "this meeting has no video source".to_string())?;
    let ffmpeg = locate_ffmpeg(root);
    let tesseract = locate_tesseract(root);
    let out_dir = root.join("derived").join(&meeting_id).join("frames");
    std::fs::create_dir_all(&out_dir).map_err(command_error)?;
    let ocr_engine = bea_core::TesseractOcrEngine {
        executable: tesseract,
        language: "eng".into(),
    };
    let mut frames = Vec::new();
    for ts in timestamps {
        let ts = *ts;
        let path = out_dir.join(format!("frame-{ts:06}.jpg"));
        if !path.exists() {
            let args = vec![
                "-y".into(),
                "-ss".into(),
                ts.to_string(),
                "-i".into(),
                video.path.to_string_lossy().into_owned(),
                "-frames:v".into(),
                "1".into(),
                "-q:v".into(),
                "3".into(),
                "-vf".into(),
                "scale=1280:-2".into(),
                path.to_string_lossy().into_owned(),
            ];
            bea_core::run_ffmpeg(&ffmpeg, &args).map_err(command_error)?;
        }
        let ocr_frame = bea_core::VisualFrame {
            id: format!("{meeting_id}-{ts}"),
            timestamp_seconds: ts,
            path: path.clone(),
            thumbnail_path: None,
            perceptual_hash: String::new(),
            description: None,
        };
        let ocr = ocr_engine
            .extract_text(&ocr_frame)
            .ok()
            .map(|result| result.text)
            .filter(|text| !text.trim().is_empty());
        frames.push(ExtractedFrame {
            timestamp_seconds: ts,
            path: path.to_string_lossy().into_owned(),
            ocr_text: ocr,
        });
    }
    Ok(frames)
}

/// Builds the OCR-context block appended to prompts when the selected model
/// cannot see images. Empty when no frame carried OCR text.
fn ocr_context_block(frames: &[ExtractedFrame]) -> String {
    let entries: Vec<String> = frames
        .iter()
        .filter_map(|frame| {
            frame
                .ocr_text
                .as_deref()
                .map(|text| format!("[{}s] {}", frame.timestamp_seconds, text.trim()))
        })
        .collect();
    if entries.is_empty() {
        String::new()
    } else {
        format!(
            "\n\n=== VISUAL CONTEXT (OCR of requested frames) ===\n{}\n",
            entries.join("\n---\n")
        )
    }
}

#[derive(serde::Serialize)]
struct PlaybackProxy {
    path: String,
    kind: String,
}

/// Ensures the meeting has a video the WebView can actually decode. mkv/avi
/// (and any other container/codec Chromium rejects) are transcoded once into
/// `derived/<meeting_id>/playback.webm` with VP9/Vorbis; the result is cached
/// so later playbacks reuse it. mp4/webm sources pass through unchanged.
#[tauri::command]
fn ensure_playable_proxy_command(
    state: State<'_, AppState>,
    meeting_id: String,
    media_path: String,
) -> Result<PlaybackProxy, String> {
    let root = state
        .database_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .to_path_buf();
    let meeting_id = sanitize_meeting_id(&meeting_id)?;
    let source = std::path::PathBuf::from(&media_path);
    if !source.is_file() {
        return Err("media file not found".into());
    }
    {
        let database = open_database(&state.database_path).map_err(command_error)?;
        verify_media_source(&database, &meeting_id, &source)?;
    }
    let extension = source
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .unwrap_or_default();
    if !matches!(extension.as_str(), "mkv" | "avi") {
        // Chromium decodes mp4/h264, mov/h264 (usually), and webm natively —
        // no proxy needed.
        return Ok(PlaybackProxy {
            path: media_path,
            kind: "video".into(),
        });
    }
    let playback = root
        .join("derived")
        .join(&meeting_id)
        .join("playback.webm");
    if !playback.exists() {
        let ffmpeg = locate_ffmpeg(&root);
        std::fs::create_dir_all(playback.parent().unwrap_or(&root)).map_err(command_error)?;
        let args = vec![
            "-y".to_string(),
            "-i".to_string(),
            source.to_string_lossy().into_owned(),
            "-c:v".to_string(),
            "libvpx-vp9".to_string(),
            "-crf".to_string(),
            "34".to_string(),
            "-b:v".to_string(),
            "0".to_string(),
            "-c:a".to_string(),
            "libvorbis".to_string(),
            playback.to_string_lossy().into_owned(),
        ];
        bea_core::run_ffmpeg(&ffmpeg, &args).map_err(command_error)?;
    }
    Ok(PlaybackProxy {
        path: playback.to_string_lossy().into_owned(),
        kind: "video".into(),
    })
}

/// Extracts the requested frames and returns them plus the OCR context block
/// for the non-vision path. Shared by chat and minutes generation.
fn gather_visual_context(
    root: &std::path::Path,
    database: &rusqlite::Connection,
    meeting_id: &str,
    timestamps: &[u64],
) -> Result<(Vec<ExtractedFrame>, String), String> {
    let frames = extract_frames_inner(root, database, meeting_id, timestamps)?;
    let ocr_block = ocr_context_block(&frames);
    Ok((frames, ocr_block))
}

/// Decodes a `data:image/...;base64,<payload>` URL into raw bytes. Used for
/// chat attachments pasted or uploaded in the UI.
fn decode_data_url(data_url: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    let payload = data_url
        .split_once(',')
        .map(|(_, payload)| payload)
        .ok_or("image payload must be a data URL")?;
    base64::engine::general_purpose::STANDARD
        .decode(payload.trim())
        .map_err(|error| format!("image data was not valid base64: {error}"))
}

#[tauri::command]
fn waveform_peaks_command(path: String, peak_count: Option<usize>) -> Result<Vec<f32>, String> {
    waveform_peaks(path, peak_count.unwrap_or(256)).map_err(command_error)
}

#[tauri::command]
fn repair_runtime_command(
    state: State<'_, AppState>,
    tools: Vec<String>,
) -> Result<RuntimeAvailability, String> {
    let root = state
        .database_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let model_path = root.join("models");
    let bin_root = root.join("bin");
    std::fs::create_dir_all(&bin_root).map_err(command_error)?;
    if tools.iter().any(|tool| tool == "ffmpeg") {
        if let Some(source) = bundled_binary_candidates("ffmpeg.exe")
            .into_iter()
            .find(|path| path.is_file())
        {
            std::fs::copy(source, bin_root.join("ffmpeg.exe")).map_err(command_error)?;
        }
        if let Some(source) = bundled_binary_candidates("ffprobe.exe")
            .into_iter()
            .find(|path| path.is_file())
        {
            std::fs::copy(source, bin_root.join("ffprobe.exe")).map_err(command_error)?;
        }
    }
    if tools.iter().any(|tool| tool == "tesseract") {
        let sources = [
            std::env::current_exe().ok().and_then(|path| {
                path.parent()
                    .map(|parent| parent.join("binaries").join("tesseract"))
            }),
            std::env::current_exe().ok().and_then(|path| {
                path.parent()
                    .map(|parent| parent.join("resources").join("binaries").join("tesseract"))
            }),
            Some(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("binaries")
                    .join("tesseract"),
            ),
        ];
        if let Some(source) = sources
            .into_iter()
            .flatten()
            .find(|path| path.join("tesseract.exe").is_file())
        {
            copy_directory(&source, &bin_root.join("tesseract"))?;
        }
    }
    let ffmpeg_path = locate_ffmpeg(root);
    let ocr_path = locate_tesseract(root);
    Ok(inspect_runtime(model_path, ffmpeg_path, ocr_path, None))
}

#[tauri::command]
async fn test_provider_connection_command(
    provider: ProviderConfig,
    api_key: String,
) -> Result<(), String> {
    if provider.base_url.trim().is_empty() || provider.model.trim().is_empty() {
        return Err("provider URL and model are required".into());
    }
    if api_key.trim().is_empty() && provider.kind.requires_api_key() {
        return Err("an API key is required for the connection test".into());
    }
    let endpoint = format!(
        "{}/chat/completions",
        provider.base_url.trim_end_matches('/')
    );
    let mut request_builder = http_client().post(endpoint);
    // Local servers (LM Studio / Ollama / llama.cpp) listen without auth.
    if !api_key.trim().is_empty() {
        request_builder = request_builder.bearer_auth(api_key);
    }
    let response = request_builder
        .json(&serde_json::json!({
            "model": provider.model,
            "messages": [{"role": "user", "content": "Reply with the single word: ready"}],
            "max_tokens": 4,
            "temperature": 0
        }))
        .send()
        .await
        .map_err(command_error)?;
    if !response.status().is_success() {
        return Err(format!("provider returned HTTP {}", response.status()));
    }
    // A 200 with a non-JSON body (HTML error page, empty body, compressed
    // bytes) must fail here, not later during minutes generation.
    let raw = response.text().await.map_err(command_error)?;
    let payload: serde_json::Value = serde_json::from_str(&raw).map_err(|_| {
        format!(
            "provider response was not valid JSON; body started with: {:?}",
            raw.chars().map(|c| if c.is_whitespace() { ' ' } else { c }).take(160).collect::<String>()
        )
    })?;
    if payload.get("choices").and_then(serde_json::Value::as_array).is_none() {
        return Err(format!(
            "provider response had no choices[] — check the model id and endpoint; body: {:?}",
            raw.chars().map(|c| if c.is_whitespace() { ' ' } else { c }).take(160).collect::<String>()
        ));
    }
    Ok(())
}

/// Preflight for minutes models: runs the real pipeline (Responses/chat
/// translation, minutes JSON schema, minutes parser) against a tiny synthetic
/// transcript, so a model that cannot produce usable minutes JSON is rejected
/// at selection time instead of after a full meeting.
#[tauri::command]
async fn validate_minutes_model_command(
    provider: ProviderConfig,
    api_key: String,
) -> Result<(), String> {
    if provider.base_url.trim().is_empty() || provider.model.trim().is_empty() {
        return Err("provider URL and model are required".into());
    }
    if api_key.trim().is_empty() && provider.kind.requires_api_key() {
        return Err("an API key is required for the minutes validation".into());
    }
    let request = LlmRequest {
        model: provider.model.clone(),
        system: MEETING_SECRETARY_SYSTEM_PROMPT.to_string(),
        user: r#"[{"start_seconds":0,"end_seconds":14,"speaker":1,"text":"We decided to ship the beta build on Friday."},{"start_seconds":14,"end_seconds":30,"speaker":2,"text":"I will prepare the release notes by Thursday. The budget review is still unresolved."}]"#.into(),
        json_schema: MINUTES_JSON_SCHEMA.into(),
        // Mirror the production minutes budget: models that pretty-print
        // (ignoring the schema's intent) need far more tokens than a compact
        // reply, and a truncated reply is unparseable.
        max_output_tokens: 8_000,
        reasoning_effort: bea_core::normalize_reasoning_effort(&provider.reasoning_effort)
            .to_string(),
    };
    bea_core::call_provider(&provider, &request, Some(&api_key))
        .await
        .map(|_| ())
        .map_err(command_error)
}

#[tauri::command]
fn search_transcript_command(
    state: State<'_, AppState>,
    meeting_id: String,
    query: String,
) -> Result<Vec<TranscriptSegment>, String> {
    let database = open_database(&state.database_path).map_err(command_error)?;
    search_transcript(&database, &meeting_id, &query).map_err(command_error)
}

#[tauri::command]
fn list_transcript_command(
    state: State<'_, AppState>,
    meeting_id: String,
) -> Result<Vec<TranscriptSegment>, String> {
    let database = open_database(&state.database_path).map_err(command_error)?;
    list_transcript(&database, &meeting_id).map_err(command_error)
}

#[tauri::command]
fn load_minutes_command(
    state: State<'_, AppState>,
    meeting_id: String,
) -> Result<Option<Minutes>, String> {
    let database = open_database(&state.database_path).map_err(command_error)?;
    load_minutes(&database, &meeting_id).map_err(command_error)
}

#[tauri::command]
fn save_minutes_command(
    state: State<'_, AppState>,
    meeting_id: String,
    minutes: Minutes,
) -> Result<(), String> {
    let database = open_database(&state.database_path).map_err(command_error)?;
    save_minutes(&database, &meeting_id, &minutes).map_err(command_error)
}

#[derive(serde::Serialize)]
struct ProviderContextPreview {
    evidence_count: usize,
    estimated_input_tokens: usize,
    disclosure: String,
}

fn keyring_secret(provider: &ProviderConfig) -> Option<String> {
    let credential_ref = provider.credential_ref.as_deref()?;
    let id = credential_ref.strip_prefix("keyring:")?;
    Entry::new("bea-provider", id).ok()?.get_password().ok()
}

const CODEX_KEYRING_ID: &str = "codex-oauth";
const CODEX_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";

/// OAuth tokens live in a user-profile JSON file instead of Windows
/// Credential Manager: the credential blob is capped at 2560 bytes (UTF-16),
/// and a single ChatGPT access token alone can exceed that, which made every
/// sign-in fail with "longer than platform limit of 2560 chars". Same
/// approach as the Codex CLI's own auth.json — outside the project database,
/// readable only by the current user.
fn codex_token_path() -> Result<PathBuf, String> {
    let base = if let Ok(app_dir) = std::env::var("BEA_APP_DATA") {
        PathBuf::from(app_dir)
    } else {
        let appdata = std::env::var("APPDATA")
            .map_err(|_| "APPDATA is not set; cannot resolve the token storage path".to_string())?;
        PathBuf::from(appdata).join("com.bea.meetingassistant")
    };
    std::fs::create_dir_all(&base).map_err(command_error)?;
    Ok(base.join("codex-auth.json"))
}

fn load_codex_tokens() -> Option<bea_core::codex_oauth::CodexTokens> {
    // Preferred store: the user-profile token file.
    if let Ok(path) = codex_token_path() {
        if let Ok(raw) = std::fs::read_to_string(&path) {
            if let Ok(tokens) = serde_json::from_str(&raw) {
                return Some(tokens);
            }
        }
    }
    // Legacy store: the original Windows Credential Manager entry. One-way
    // migration keeps users who signed in before the switch signed in.
    let raw = Entry::new("bea-provider", CODEX_KEYRING_ID).ok()?.get_password().ok()?;
    let tokens: bea_core::codex_oauth::CodexTokens = serde_json::from_str(&raw).ok()?;
    if let (Ok(path), Ok(serialized)) =
        (codex_token_path(), serde_json::to_string(&tokens))
    {
        if std::fs::write(&path, serialized).is_ok() {
            if let Ok(entry) = Entry::new("bea-provider", CODEX_KEYRING_ID) {
                let _ = entry.delete_credential();
            }
        }
    }
    Some(tokens)
}

fn store_codex_tokens(tokens: &bea_core::codex_oauth::CodexTokens) -> Result<(), String> {
    let path = codex_token_path()?;
    let serialized = serde_json::to_string(tokens).map_err(command_error)?;
    // Atomic write: a crash mid-write must not corrupt the only token copy.
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, serialized).map_err(command_error)?;
    std::fs::rename(&temporary, &path).map_err(command_error)?;
    // Remove any oversized legacy Credential Manager entry; it can never be
    // read successfully and only leaves stale tokens behind.
    if let Ok(entry) = Entry::new("bea-provider", CODEX_KEYRING_ID) {
        let _ = entry.delete_credential();
    }
    Ok(())
}

async fn exchange_codex_code(
    code: &str,
    verifier: &str,
) -> Result<bea_core::codex_oauth::CodexTokens, String> {
    let response = http_client()
        .post(CODEX_TOKEN_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(bea_core::codex_oauth::token_exchange_body(code, verifier))
        .send()
        .await
        .map_err(command_error)?
        .error_for_status()
        .map_err(command_error)?;
    let payload: serde_json::Value = response.json().await.map_err(command_error)?;
    let account_id = payload
        .get("https://api.openai.com/auth")
        .and_then(|value| value.get("chatgpt_account_id"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    Ok(bea_core::codex_oauth::CodexTokens {
        access_token: payload["access_token"]
            .as_str()
            .ok_or("token response missing access_token")?
            .to_string(),
        refresh_token: payload["refresh_token"]
            .as_str()
            .ok_or("token response missing refresh_token")?
            .to_string(),
        expires_at: chrono::Utc::now().timestamp()
            + payload["expires_in"]
                .as_i64()
                .ok_or("token response missing expires_in")?,
        account_id,
    })
}

/// Returns a valid access token, refreshing via the stored refresh token when
/// the current one is within 60 seconds of expiry. A global mutex serializes
/// the refresh flow: concurrent callers would otherwise burn the single-use
/// refresh token twice; the second caller re-reads the freshly stored token
/// after the first refresh completes.
async fn fresh_codex_access_token() -> Result<String, String> {
    static REFRESH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _guard = REFRESH_LOCK.lock().await;
    let tokens =
        load_codex_tokens().ok_or("no ChatGPT sign-in — click Sign in with ChatGPT first")?;
    if tokens.expires_at - 60 > chrono::Utc::now().timestamp() {
        return Ok(tokens.access_token);
    }
    let response = http_client()
        .post(CODEX_TOKEN_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(bea_core::codex_oauth::token_refresh_body(
            &tokens.refresh_token,
        ))
        .send()
        .await
        .map_err(command_error)?
        .error_for_status()
        .map_err(command_error)?;
    let payload: serde_json::Value = response.json().await.map_err(command_error)?;
    let updated = bea_core::codex_oauth::CodexTokens {
        access_token: payload["access_token"]
            .as_str()
            .ok_or("refresh response missing access_token")?
            .to_string(),
        refresh_token: payload["refresh_token"]
            .as_str()
            .map(str::to_string)
            .unwrap_or(tokens.refresh_token.clone()),
        expires_at: chrono::Utc::now().timestamp() + payload["expires_in"].as_i64().unwrap_or(3600),
        account_id: tokens.account_id.clone(),
    };
    store_codex_tokens(&updated)?;
    Ok(updated.access_token)
}

/// Parses the `code` and `state` query parameters from an OAuth callback
/// request line. The query string starts at the path's `?` only — the old
/// `split("code=")` also matched inside path/other params.
fn parse_oauth_callback(
    request_line: &str,
    expected_state: &str,
) -> Result<String, String> {
    let path = request_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| "malformed HTTP request line".to_string())?;
    let query = path
        .split_once('?')
        .map(|(_, query)| query)
        .ok_or_else(|| "callback contained no query string".to_string())?;
    let mut code: Option<String> = None;
    let mut state_matched = false;
    for pair in query.split('&') {
        let Some((name, value)) = pair.split_once('=') else {
            continue;
        };
        if name == "state" {
            state_matched = value == expected_state;
        } else if name == "code" && code.is_none() {
            code = Some(value.to_string());
        }
    }
    if !state_matched {
        return Err("OAuth state mismatch — login attempt rejected".into());
    }
    code.ok_or_else(|| "callback contained no authorization code".to_string())
}

fn generate_oauth_state() -> String {
    Uuid::new_v4().to_string()
}

#[tauri::command]
async fn codex_oauth_login_command() -> Result<(), String> {
    use std::io::Read;
    let verifier = bea_core::codex_oauth::pkce_verifier();
    let state = generate_oauth_state();
    let url = format!(
        "{}&state={}",
        bea_core::codex_oauth::authorize_url(&verifier),
        state
    );
    // Open the default browser on the Windows host. `cmd /C start` treats the
    // URL's `&` characters as command separators — the browser then only ever
    // sees `?response_type=code` and auth.openai.com answers with
    // missing_required_parameter. rundll32's FileProtocolHandler passes the
    // URL through untouched (verified with a local callback-server probe).
    std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", &url])
        .spawn()
        .map_err(|e| format!("could not open the browser: {e}"))?;
    let code = tauri::async_runtime::spawn_blocking(move || -> Result<String, String> {
        let listener = std::net::TcpListener::bind("127.0.0.1:1455")
            .map_err(|e| format!("port 1455 unavailable (another login is running?): {e}"))?;
        let (mut stream, _) = listener.accept().map_err(|e| e.to_string())?;
        let mut buffer = [0u8; 4096];
        let read = stream.read(&mut buffer).map_err(|e| e.to_string())?;
        // Request line: GET /auth/callback?code=...&state=... HTTP/1.1
        let request = String::from_utf8_lossy(&buffer[..read]).to_string();
        let line = request.lines().next().unwrap_or_default();
        let outcome = parse_oauth_callback(line, &state);
        // The page the user sees must state the truth: a failed callback
        // (state mismatch, missing code) renders the error variant instead of
        // a blanket "connected".
        let (status, html) = match &outcome {
            Ok(_) => (
                "HTTP/1.1 200 OK\r\n",
                bea_core::codex_oauth::callback_page_html(
                    true,
                    "You're connected",
                    "Bea is now signed in to your ChatGPT account.",
                ),
            ),
            Err(error) => (
                "HTTP/1.1 400 Bad Request\r\n",
                bea_core::codex_oauth::callback_page_html(
                    false,
                    "Sign-in didn't finish",
                    &format!("{error}."),
                ),
            ),
        };
        let _ = stream.write_all(
            format!(
                "{status}Content-Type: text/html; charset=utf-8\r\nConnection: close\r\n\r\n{html}"
            )
            .as_bytes(),
        );
        outcome
    })
    .await
    .map_err(|e| e.to_string())??;
    let tokens = exchange_codex_code(&code, &verifier).await?;
    store_codex_tokens(&tokens)
}

#[tauri::command]
fn codex_oauth_status_command() -> Result<bool, String> {
    Ok(load_codex_tokens()
        .map(|t| t.expires_at - 60 > chrono::Utc::now().timestamp())
        .unwrap_or(false))
}

#[tauri::command]
fn codex_oauth_sign_out_command() -> Result<(), String> {
    if let Ok(entry) = Entry::new("bea-provider", CODEX_KEYRING_ID) {
        let _ = entry.delete_credential();
    }
    Ok(())
}

#[tauri::command]
async fn codex_list_models_command() -> Result<Vec<String>, String> {
    // Mirrors what the Codex CLI sends when it lists models: bearer token,
    // the account header, and a client_version query. Without the account
    // header the backend answers with an empty/generic catalog.
    let tokens = load_codex_tokens()
        .ok_or("no ChatGPT sign-in — click Sign in with ChatGPT first")?;
    let token = fresh_codex_access_token().await?;
    let mut request = http_client()
        .get(format!(
            "{}/models?client_version=0.149.0",
            bea_core::codex_oauth::CHATGPT_API_BASE
        ))
        .bearer_auth(token);
    if !tokens.account_id.trim().is_empty() {
        request = request.header("ChatGPT-Account-Id", tokens.account_id.trim());
    }
    let response = request.send().await.map_err(command_error)?;
    // The Codex backend may not publish a catalog; an empty list tells the
    // UI to fall back to the static model list.
    if !response.status().is_success() {
        return Ok(vec![]);
    }
    let payload: serde_json::Value = response.json().await.map_err(command_error)?;
    Ok(payload
        .get("data")
        .and_then(|value| value.as_array())
        .map(|models| {
            models
                .iter()
                .filter_map(|model| {
                    model
                        .get("id")
                        .and_then(|id| id.as_str())
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default())
}

#[tauri::command]
fn preview_provider_context_command(
    state: State<'_, AppState>,
    meeting_id: String,
) -> Result<ProviderContextPreview, String> {
    let database = open_database(&state.database_path).map_err(command_error)?;
    let transcript = list_transcript(&database, &meeting_id).map_err(command_error)?;
    let events = extract_ledger_events(&transcript);
    let pack = bea_core::pack_context_mode(&events, 12_000, ContextMode::Balanced);
    Ok(ProviderContextPreview {
        evidence_count: pack.evidence_count,
        estimated_input_tokens: pack.estimated_input_tokens,
        disclosure: "Only packed transcript text and timestamped evidence will be sent. Raw audio and video stay local.".into(),
    })
}

#[tauri::command]
async fn generate_minutes_command(
    state: State<'_, AppState>,
    meeting_id: String,
) -> Result<Minutes, String> {
    // A fresh run clears any stale cancellation flag from a previous attempt.
    if let Ok(mut cancelled) = state.cancelled_minutes.lock() {
        cancelled.remove(&meeting_id);
    }
    let database = open_database(&state.database_path).map_err(command_error)?;
    let meeting = list_meetings(&database)
        .map_err(command_error)?
        .into_iter()
        .find(|meeting| meeting.id == meeting_id)
        .ok_or_else(|| "meeting not found".to_string())?;
    let transcript = list_transcript(&database, &meeting_id).map_err(command_error)?;
    if transcript.is_empty() {
        return Err("a transcript is required before minutes can be generated".to_string());
    }
    let events = extract_ledger_events(&transcript);
    save_ledger_events(&database, &meeting_id, &events).map_err(command_error)?;
    let local_minutes = generate_minutes(&meeting.title, &events);
    let mut minutes = local_minutes.clone();
    if let Some(provider) = load_provider(&database, "primary").map_err(command_error)? {
        if provider.enabled {
            // Local servers take no auth; OAuth providers resolve a fresh
            // access token; keyring providers read their API key.
            let api_key = if provider.kind == bea_core::ProviderKind::OpenAiOAuth {
                // A failed refresh must surface, not silently fall back to
                // local minutes — the user thinks OAuth minutes worked.
                Some(fresh_codex_access_token().await?)
            } else {
                keyring_secret(&provider)
            };
            if let Some(api_key) = api_key {
                let speaker_names =
                    bea_core::list_speaker_names(&database, &meeting_id).unwrap_or_default();
                let custom_format = bea_core::get_app_setting(
                    &database,
                    &format!("custom_minutes_format:{meeting_id}"),
                )
                .map_err(command_error)?;
                let user_notes = list_context_events_payload(&database, &meeting_id)?;
                let mut meeting_context =
                    bea_core::build_meeting_context(&speaker_names, custom_format.as_deref());
                meeting_context.push_str(&format!("\n=== USER CLARIFICATIONS & CONTEXT (treat as authoritative) ===\n{user_notes}\n"));
                // Prefill agenda on regenerate: if minutes already exist, surface
                // their agenda so the model preserves/extends it instead of
                // returning an empty list.
                if let Ok(Some(previous)) = bea_core::load_minutes(&database, &meeting_id) {
                    if !previous.agenda.is_empty() {
                        let prev_headings = previous.agenda.iter().map(|a| format!("- {}", a.heading)).collect::<Vec<_>>().join("\n");
                        meeting_context.push_str(&format!("\n=== PREVIOUS AGENDA (prefill — keep, refine, or extend; do not discard without reason) ===\n{prev_headings}\n"));
                    }
                }
                // Visual context: when the meeting has a video, the AI pulls
                // frames itself — evenly spaced across the video (vision
                // models see the images; text-only models get their OCR text).
                // No manual timestamp input.
                let mut vision_images: Vec<(std::path::PathBuf, String)> = Vec::new();
                {
                    let has_video = list_media_command_inner(&database, &meeting_id)?
                        .iter()
                        .any(|media| matches!(media.kind, MediaKind::Video));
                    if has_video {
                        let duration = meeting.duration_seconds.max(1);
                        let count = duration.clamp(3, 8);
                        let step = duration / count;
                        let timestamps: Vec<u64> = (0..count)
                            .map(|index| index * step + step / 2)
                            .collect();
                        let root = state
                            .database_path
                            .parent()
                            .unwrap_or_else(|| std::path::Path::new("."))
                            .to_path_buf();
                        let (frames, ocr_block) = gather_visual_context(
                            &root,
                            &database,
                            &meeting_id,
                            &timestamps,
                        )?;
                        if meeting_vision_capable(
                            &database,
                            &meeting_id,
                            &provider.kind,
                            &provider.model,
                        ) && !frames.is_empty()
                        {
                            vision_images = frames
                                .iter()
                                .map(|frame| (std::path::PathBuf::from(&frame.path), String::new()))
                                .collect();
                        } else if !ocr_block.trim().is_empty() {
                            meeting_context.push_str(&format!(
                                "=== VISUAL CONTEXT (OCR of video frames, auto-selected) ===\n{ocr_block}\n"
                            ));
                        }
                    }
                }
                let pack = bea_core::pack_context_mode(&events, 12_000, ContextMode::Balanced);
                let effective_model =
                    effective_meeting_model(&database, &meeting_id, &provider.model);
                let effective_reasoning =
                    effective_meeting_reasoning(&database, &meeting_id, &provider.reasoning_effort);
                let request = LlmRequest {
                    model: effective_model,
                    system: format!("{}\n{}", MEETING_SECRETARY_SYSTEM_PROMPT, meeting_context),
                    user: serde_json::to_string(&pack.events).map_err(command_error)?,
                    json_schema: MINUTES_JSON_SCHEMA.into(),
                    max_output_tokens: MINUTES_MAX_OUTPUT_TOKENS,
                    reasoning_effort: effective_reasoning,
                };
                // Cancellation checkpoint: the wizard's Close stops the run
                // before the (possibly long) provider request is even sent.
                let was_cancelled = || {
                    state
                        .cancelled_minutes
                        .lock()
                        .map(|set| set.contains(&meeting_id))
                        .unwrap_or(false)
                };
                if was_cancelled() {
                    return Err("cancelled: minutes generation was closed before it finished".into());
                }
                let remote_result = if vision_images.is_empty() {
                    call_provider(&provider, &request, Some(&api_key)).await
                } else {
                    // Vision path: send the frame images inline and parse the
                    // minutes JSON out of the free-text reply. A malformed
                    // reply goes back for bounded self-repair before failing.
                    match bea_core::call_provider_messages(
                        &provider,
                        &request.system,
                        &request.user,
                        &vision_images,
                        request.max_output_tokens,
                        &request.reasoning_effort,
                        Some(&api_key),
                        &request.model,
                    )
                    .await
                    {
                        Ok(text) => match bea_core::parse_minutes_json(&text) {
                            Ok(minutes) => Ok(minutes),
                            Err(error) => {
                                bea_core::repair_minutes_reply(
                                    &provider,
                                    &request,
                                    Some(&api_key),
                                    text,
                                    error,
                                )
                                .await
                            }
                        },
                        Err(error) => Err(error),
                    }
                };
                match remote_result {
                    Ok(remote_minutes) => {
                        // The reply arrived after the user closed the wizard:
                        // discard it instead of saving half-wanted minutes.
                        if was_cancelled() {
                            return Err("cancelled: minutes generation was closed before it finished".into());
                        }
                        let output_tokens = estimate_tokens(
                            &serde_json::to_string(&remote_minutes).unwrap_or_default(),
                        ) as u64;
                        record_usage(
                            &database,
                            &UsageRecord {
                                provider_id: provider.id,
                                model: provider.model,
                                input_tokens: pack.estimated_input_tokens as u64,
                                output_tokens,
                                estimated_cost: None,
                                operation: "minutes".into(),
                            },
                        )
                        .map_err(command_error)?;
                        minutes = remote_minutes;
                    }
                    Err(provider_error) => {
                        // A configured provider that fails must not silently
                        // degrade to heuristic minutes (empty agenda, verbatim
                        // transcript lines posing as summaries) — surface the
                        // failure so the user can fix the provider instead of
                        // mistaking fallback output for real AI minutes.
                        return Err(minutes_failure_message(&provider_error.to_string()));
                    }
                }
            }
        }
    }
    // Final checkpoint before persistence — the local-heuristic path reaches
    // this too when no provider is configured.
    if state
        .cancelled_minutes
        .lock()
        .map(|set| set.contains(&meeting_id))
        .unwrap_or(false)
    {
        return Err("cancelled: minutes generation was closed before it finished".into());
    }
    save_minutes(&database, &meeting_id, &minutes).map_err(command_error)?;
    Ok(minutes)
}

/// Marks an in-flight minutes generation as cancelled. The next checkpoint in
/// `generate_minutes_command` (before the provider request, after the reply,
/// and before saving) aborts the run, so nothing overwrites the saved minutes.
#[tauri::command]
fn cancel_minutes_generation_command(
    state: State<'_, AppState>,
    meeting_id: String,
) -> Result<(), String> {
    if let Ok(mut cancelled) = state.cancelled_minutes.lock() {
        cancelled.insert(meeting_id);
    }
    Ok(())
}

#[tauri::command]
async fn modify_minutes_command(
    state: State<'_, AppState>,
    meeting_id: String,
    instruction: String,
) -> Result<bea_core::Minutes, String> {
    if instruction.trim().is_empty() {
        return Err("an instruction is required".into());
    }
    let database = open_database(&state.database_path).map_err(command_error)?;
    let meeting_id = sanitize_meeting_id(&meeting_id)?;
    let provider = load_provider(&database, "primary")
        .map_err(command_error)?
        .filter(|provider| provider.enabled)
        .ok_or_else(|| "a verified AI provider is required to modify minutes".to_string())?;
    let api_key = provider_key(&provider).await?;
    let current = bea_core::load_minutes(&database, &meeting_id)
        .map_err(command_error)?
        .ok_or_else(|| "generate minutes first — there is nothing to modify yet".to_string())?;
    let context = list_context_events_payload(&database, &meeting_id)?;
    let effective_model =
        effective_meeting_model(&database, &meeting_id, &provider.model);
    let effective_reasoning =
        effective_meeting_reasoning(&database, &meeting_id, &provider.reasoning_effort);

    let system = format!(
        "{}\n\nThe user reviewed the minutes below and asks for a change. Apply EXACTLY the requested change, keep everything else verbatim (including timestamps and evidence quotes), and return the complete updated JSON matching the schema.\n\nUser instruction: {instruction}\n\nContext notes (clarifications and facts to honor):\n{context}",
        MEETING_SECRETARY_SYSTEM_PROMPT
    );
    let request = LlmRequest {
        model: effective_model,
        system,
        user: serde_json::to_string(&current).map_err(command_error)?,
        json_schema: r#"{"type":"object","properties":{"title":{"type":"string"},"summary":{"type":"string"},"agenda":{"type":"array","items":{"type":"object","properties":{"heading":{"type":"string"},"start_seconds":{"type":"number"},"end_seconds":{"type":"number"}},"required":["heading"]}},"visual_observations":{"type":"array","items":{"type":"string"}},"decisions":{"type":"array"},"action_items":{"type":"array"},"unresolved":{"type":"array"}},"required":["title","summary","decisions","action_items"]}"#.to_string(),
        max_output_tokens: 4096,
        reasoning_effort: effective_reasoning,
    };
    let text = bea_core::call_provider_text(&provider, &request, Some(&api_key))
        .await
        .map_err(command_error)?;
    let minutes = bea_core::parse_minutes_json(&text).map_err(command_error)?;
    bea_core::save_minutes(&database, &meeting_id, &minutes).map_err(command_error)?;
    Ok(minutes)
}

/// Resolves the API key for a provider (OAuth refresh or keyring), shared by
/// the minutes/chat commands.
async fn provider_key(provider: &ProviderConfig) -> Result<String, String> {
    if provider.kind == bea_core::ProviderKind::OpenAiOAuth {
        fresh_codex_access_token().await
    } else {
        keyring_secret(provider)
            .ok_or_else(|| "provider API key is missing — re-verify the connection in Settings".to_string())
    }
}

#[tauri::command]
fn set_speaker_name_command(
    state: State<'_, AppState>,
    meeting_id: String,
    speaker_index: u32,
    name: String,
) -> Result<(), String> {
    let database = open_database(&state.database_path).map_err(command_error)?;
    bea_core::set_speaker_name(&database, &meeting_id, speaker_index, &name).map_err(command_error)
}

#[tauri::command]
fn list_speaker_names_command(
    state: State<'_, AppState>,
    meeting_id: String,
) -> Result<Vec<(u32, String)>, String> {
    let database = open_database(&state.database_path).map_err(command_error)?;
    let mut names: Vec<(u32, String)> = bea_core::list_speaker_names(&database, &meeting_id)
        .map_err(command_error)?
        .into_iter()
        .collect();
    names.sort();
    Ok(names)
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ContextEventRow {
    id: String,
    meeting_id: String,
    kind: String,
    payload: String,
    created_at: String,
}

fn add_context_event_command_inner(
    database: &rusqlite::Connection,
    meeting_id: &str,
    kind: &str,
    payload: &str,
) -> Result<ContextEventRow, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let created_at = chrono::Utc::now().to_rfc3339();
    database
        .execute(
            "INSERT INTO context_events(id,meeting_id,kind,payload,confidence,created_at) VALUES (?1,?2,?3,?4,1.0,?5)",
            rusqlite::params![id, meeting_id, kind, payload, created_at],
        )
        .map_err(command_error)?;
    Ok(ContextEventRow {
        id,
        meeting_id: meeting_id.into(),
        kind: kind.into(),
        payload: payload.into(),
        created_at,
    })
}

#[tauri::command]
fn add_context_event_command(
    state: State<'_, AppState>,
    meeting_id: String,
    kind: String,
    payload: String,
) -> Result<ContextEventRow, String> {
    if !matches!(kind.as_str(), "clarify" | "context" | "chat") {
        return Err("kind must be clarify, context, or chat".into());
    }
    let database = open_database(&state.database_path).map_err(command_error)?;
    add_context_event_command_inner(&database, &meeting_id, &kind, payload.trim())
}

#[tauri::command]
fn list_context_events_command(
    state: State<'_, AppState>,
    meeting_id: String,
) -> Result<Vec<ContextEventRow>, String> {
    let database = open_database(&state.database_path).map_err(command_error)?;
    let mut statement = database
        .prepare("SELECT id,meeting_id,kind,payload,created_at FROM context_events WHERE meeting_id=?1 ORDER BY created_at, rowid")
        .map_err(command_error)?;
    let rows = statement
        .query_map(rusqlite::params![meeting_id], |row| {
            Ok(ContextEventRow {
                id: row.get(0)?,
                meeting_id: row.get(1)?,
                kind: row.get(2)?,
                payload: row.get(3)?,
                created_at: row.get(4)?,
            })
        })
        .map_err(command_error)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(command_error)
}

#[tauri::command]
fn delete_context_event_command(
    state: State<'_, AppState>,
    event_id: String,
) -> Result<(), String> {
    let database = open_database(&state.database_path).map_err(command_error)?;
    database
        .execute(
            "DELETE FROM context_events WHERE id=?1",
            rusqlite::params![event_id],
        )
        .map_err(command_error)?;
    Ok(())
}

/// Wipes every meeting's AI memory (chat Q&A history + clarify/context notes).
/// Transcripts, minutes, and meetings are kept. Returns the removed row count.
#[tauri::command]
fn clear_ai_memory_command(state: State<'_, AppState>) -> Result<usize, String> {
    let database = open_database(&state.database_path).map_err(command_error)?;
    let removed = database
        .execute("DELETE FROM context_events", [])
        .map_err(command_error)?;
    Ok(removed)
}

#[tauri::command]
fn apply_mass_correction_command(
    state: State<'_, AppState>,
    meeting_id: String,
    find_text: String,
    replace_text: String,
) -> Result<u32, String> {
    if find_text.trim().is_empty() {
        return Err("find text is required".into());
    }
    if find_text == replace_text {
        return Err("find and replace text are identical".into());
    }
    let mut database = open_database(&state.database_path).map_err(command_error)?;
    // Atomic FTS rebuild plus exact-case find/replace: LIKE is
    // case-insensitive while REPLACE is case-sensitive, so `instr` (case-
    // sensitive, exact) is used to match REPLACE's semantics.
    let transaction = database.transaction().map_err(command_error)?;
    let changed = transaction
        .execute(
            "UPDATE transcript_segments SET text=REPLACE(text,?1,?2) WHERE meeting_id=?3 AND instr(text,?1)>0",
            rusqlite::params![find_text, replace_text, meeting_id],
        )
        .map_err(command_error)? as u32;
    // Rebuild this meeting's FTS rows so search stays consistent.
    transaction
        .execute(
            "DELETE FROM transcript_fts WHERE meeting_id=?1",
            rusqlite::params![meeting_id],
        )
        .map_err(command_error)?;
    transaction
        .execute(
            "INSERT INTO transcript_fts(meeting_id,segment_id,text) SELECT meeting_id,id,text FROM transcript_segments WHERE meeting_id=?1 AND TRIM(text) != '[silence]'",
            rusqlite::params![meeting_id],
        )
        .map_err(command_error)?;
    transaction.commit().map_err(command_error)?;
    Ok(changed)
}

/// Assembles the chat system prompt. `memory_block` comes from
/// `build_memory_block` (empty when there are no other meetings) so questions
/// referencing earlier meetings get the index + FTS passages injected.
fn chat_system_prompt(
    meeting_context: &str,
    notes: &str,
    ledger: &str,
    raw_transcript: &str,
    ocr_block: &str,
    memory_block: &str,
) -> String {
    format!(
        "You are Bea, an assistant answering questions about one meeting.\n{meeting_context}\nAnswer using ONLY the transcript, notes, memory of other meetings, and any visual context below. Cite speaker names and timestamps. If the answer is not in the material, say so plainly.\n\n=== MEETING NOTES (user-added clarifications/context) ===\n{notes}\n\n=== MEETING LEDGER ===\n{ledger}\n\n=== FULL TRANSCRIPT ===\n{raw_transcript}{ocr_block}{memory_block}"
    )
}

#[tauri::command]
async fn chat_command(
    state: State<'_, AppState>,
    meeting_id: String,
    question: String,
    images: Option<Vec<String>>,
) -> Result<serde_json::Value, String> {
    if question.trim().is_empty() {
        return Err("a question is required".into());
    }
    let database = open_database(&state.database_path).map_err(command_error)?;
    let provider = load_provider(&database, "primary")
        .map_err(command_error)?
        .filter(|provider| provider.enabled);
    let provider =
        provider.ok_or_else(|| "a verified provider is required for chat".to_string())?;
    let api_key = if provider.kind == bea_core::ProviderKind::OpenAiOAuth {
        fresh_codex_access_token().await?
    } else {
        keyring_secret(&provider).ok_or_else(|| {
            "provider API key is missing — re-verify the connection in Settings".to_string()
        })?
    };
    let transcript = list_transcript(&database, &meeting_id).map_err(command_error)?;
    if transcript.is_empty() {
        return Err("there is no transcript to ask about yet".into());
    }
    let speaker_names = bea_core::list_speaker_names(&database, &meeting_id).unwrap_or_default();
    let custom_format = None; // chat does not need the format block
    let meeting_context = bea_core::build_meeting_context(&speaker_names, custom_format);
    let events = extract_ledger_events(&transcript);
    let pack = bea_core::pack_context_mode(&events, 12_000, ContextMode::Balanced);
    // Transcript dump with speaker names; overlapping-speech segments show
    // every attached speaker (e.g. "Maria + John").
    let overlaps = bea_core::list_segment_speakers(&database, &meeting_id).unwrap_or_default();
    let raw_transcript = transcript
        .iter()
        .filter(|segment| segment.text.trim() != "[silence]")
        .map(|segment| {
            let who = segment
                .speaker
                .and_then(|index| speaker_names.get(&index))
                .map(|name| name.as_str())
                .unwrap_or("Speaker ?");
            let extra = overlaps
                .get(&segment.id)
                .map(|indexes| {
                    indexes
                        .iter()
                        .filter(|index| Some(**index) != segment.speaker)
                        .filter_map(|index| speaker_names.get(index))
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let who = if extra.is_empty() {
                who.to_string()
            } else {
                format!("{who} + {}", extra.join(" + "))
            };
            format!(
                "[{:02}:{:02}] {}: {}",
                segment.start_seconds / 60,
                segment.start_seconds % 60,
                who,
                segment.text.trim()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    // Visual context: when the meeting has a video, the AI pulls frames itself
    // to strengthen its answer — either as images (vision models) or as OCR
    // text (fallback). Frames dedupe via the disk cache. No manual timestamps.
    let mut vision_images: Vec<(std::path::PathBuf, String)> = Vec::new();
    let mut ocr_block = String::new();
    let mut mode = String::new();
    let mut frames_used = 0usize;
    {
        let has_video = list_media_command_inner(&database, &meeting_id)?
            .iter()
            .any(|media| matches!(media.kind, MediaKind::Video));
        if has_video {
            let duration = transcript
                .last()
                .map(|segment| segment.end_seconds)
                .unwrap_or(0)
                .max(1);
            let count = duration.clamp(3, 8);
            let step = duration / count;
            let timestamps: Vec<u64> = (0..count).map(|index| index * step + step / 2).collect();
            let root = state
                .database_path
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .to_path_buf();
            let (frames, block) =
                gather_visual_context(&root, &database, &meeting_id, &timestamps)?;
            let vision_capable = meeting_vision_capable(
                &database,
                &meeting_id,
                &provider.kind,
                &provider.model,
            );
            // Only claim frames/mode when the request actually carries visual
            // content: vision mode counts the images attached; the OCR fallback
            // only counts when OCR produced a non-empty block (a missing
            // tesseract binary must not show a "frames used" chip).
            if vision_capable && !frames.is_empty() {
                frames_used = frames.len();
                mode = "vision".into();
                vision_images = frames
                    .iter()
                    .map(|frame| (std::path::PathBuf::from(&frame.path), String::new()))
                    .collect();
            } else if !block.trim().is_empty() {
                frames_used = frames.len();
                mode = "ocr".into();
                ocr_block = block;
            }
        }
    }
    // User-attached images (pasted or uploaded in the Chat panel). Vision
    // models receive the actual image; text-only models get a local Tesseract
    // OCR pass instead so the content still reaches the model.
    let attachments = images.unwrap_or_default();
    if !attachments.is_empty() {
        let root = state
            .database_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .to_path_buf();
        let safe_meeting_id = sanitize_meeting_id(&meeting_id)?;
        let dir = root
            .join("derived")
            .join(&safe_meeting_id)
            .join("chat-images");
        std::fs::create_dir_all(&dir).map_err(command_error)?;
        let vision = meeting_vision_capable(
            &database,
            &meeting_id,
            &provider.kind,
            &provider.model,
        );
        let engine = bea_core::TesseractOcrEngine {
            executable: locate_tesseract(&root),
            language: "eng".into(),
        };
        let stamp = chrono::Utc::now().timestamp_millis();
        let mut ocr_parts: Vec<String> = Vec::new();
        for (index, data_url) in attachments.iter().enumerate() {
            let bytes = decode_data_url(data_url)?;
            let path = dir.join(format!("attach-{stamp}-{}.png", index + 1));
            std::fs::write(&path, &bytes).map_err(command_error)?;
            if vision {
                vision_images.push((path, String::new()));
            } else {
                let frame = bea_core::VisualFrame {
                    id: format!("{meeting_id}-chat-{stamp}-{}", index + 1),
                    timestamp_seconds: 0,
                    path: path.clone(),
                    thumbnail_path: None,
                    perceptual_hash: String::new(),
                    description: None,
                };
                if let Ok(result) = engine.extract_text(&frame) {
                    let text = result.text.trim();
                    if !text.is_empty() {
                        ocr_parts.push(format!("[attached image {}]\n{}", index + 1, text));
                    }
                }
            }
        }
        if vision {
            frames_used += attachments.len();
            mode = "vision".into();
        } else if !ocr_parts.is_empty() {
            frames_used += attachments.len();
            mode = "ocr".into();
            ocr_block.push_str(&format!(
                "\n\n=== ATTACHED IMAGES (OCR text read locally with Tesseract) ===\n{}\n",
                ocr_parts.join("\n---\n")
            ));
        }
    }
    // Cross-meeting memory: index of every other meeting plus FTS-matched
    // transcript passages relevant to this question (empty without others).
    let memory_block = build_memory_block(&database, &meeting_id, question.trim())?;
    let notes = list_context_events_payload(&database, &meeting_id)?;
    let ledger = serde_json::to_string(&pack.events).map_err(command_error)?;
    let system = chat_system_prompt(
        &meeting_context,
        &notes,
        &ledger,
        &raw_transcript,
        &ocr_block,
        &memory_block,
    );
    let effective_reasoning =
        effective_meeting_reasoning(&database, &meeting_id, &provider.reasoning_effort);
    let effective_model = effective_meeting_model(&database, &meeting_id, &provider.model);
    let answer = if vision_images.is_empty() {
        let request = bea_core::LlmRequest {
            model: effective_model,
            system,
            user: question.trim().to_string(),
            json_schema: String::new(),
            max_output_tokens: 1_500,
            reasoning_effort: effective_reasoning,
        };
        bea_core::call_provider_text(&provider, &request, Some(&api_key))
            .await
            .map_err(command_error)?
    } else {
        // Vision path resolves the same effort as the text path above.
        bea_core::call_provider_messages(
            &provider,
            &system,
            question.trim(),
            &vision_images,
            1_500,
            &effective_reasoning,
            Some(&api_key),
            &effective_model,
        )
        .await
        .map_err(command_error)?
    };
    // Persist the Q&A pair so it survives restarts and feeds future minutes.
    let asked = if attachments.is_empty() {
        question.trim().to_string()
    } else {
        format!("{} [{} image{} attached]", question.trim(), attachments.len(), if attachments.len() == 1 { "" } else { "s" })
    };
    add_context_event_command_inner(
        &database,
        &meeting_id,
        "chat",
        &format!("Q: {}\nA: {}", asked, answer),
    )?;
    Ok(serde_json::json!({
        "answer": answer,
        "frames_used": frames_used,
        "mode": mode,
    }))
}

#[derive(serde::Serialize, Clone)]
struct CorrectionPair {
    find: String,
    replace: String,
}

#[derive(serde::Serialize, Clone)]
struct CorrectionPlan {
    replacements: Vec<CorrectionPair>,
    note: String,
}

/// Resolves a natural-language correction instruction ("replace all "enyu"
/// with "NU"") into concrete find/replace pairs using the transcript as
/// grounding, so the AI can see the actual spellings before proposing edits.
/// The pairs are then applied verbatim by apply_mass_correction_command —
/// the model never edits the transcript directly.
#[tauri::command]
async fn resolve_correction_command(
    state: State<'_, AppState>,
    meeting_id: String,
    instruction: String,
) -> Result<CorrectionPlan, String> {
    if instruction.trim().is_empty() {
        return Err("a correction instruction is required".into());
    }
    let database = open_database(&state.database_path).map_err(command_error)?;
    let provider = load_provider(&database, "primary")
        .map_err(command_error)?
        .filter(|provider| provider.enabled);
    let provider = provider
        .ok_or_else(|| "a verified provider is required for corrections".to_string())?;
    let api_key = if provider.kind == bea_core::ProviderKind::OpenAiOAuth {
        fresh_codex_access_token().await?
    } else {
        keyring_secret(&provider).ok_or_else(|| "provider API key is missing".to_string())?
    };
    let transcript = list_transcript(&database, &meeting_id).map_err(command_error)?;
    if transcript.is_empty() {
        return Err("there is no transcript to correct yet".into());
    }
    let speaker_names = bea_core::list_speaker_names(&database, &meeting_id).unwrap_or_default();
    let meeting_context = bea_core::build_meeting_context(&speaker_names, None);
    // Ground the model in the real text: unique words that actually appear,
    // plus a truncated sample of segments, so it proposes replacements that
    // match the transcript verbatim instead of guessing spellings.
    let word_counts: std::collections::HashMap<String, usize> = {
        let mut counts = std::collections::HashMap::new();
        for segment in &transcript {
            for word in segment.text.split_whitespace() {
                let cleaned: String = word
                    .chars()
                    .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '\'')
                    .collect::<String>()
                    .to_lowercase();
                if cleaned.len() >= 3 {
                    *counts.entry(cleaned).or_insert(0) += 1;
                }
            }
        }
        counts
    };
    let mut known_words: Vec<&String> = word_counts.keys().collect();
    known_words.sort();
    let word_list = known_words
        .iter()
        .map(|word| word.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let sample = transcript
        .iter()
        .take(200)
        .map(|segment| segment.text.trim())
        .collect::<Vec<_>>()
        .join("\n");
    let system = format!(
        "You convert a user's natural-language transcript-correction request into exact find/replace operations.\n{meeting_context}\nRules:\n- Ground every `find` string in the transcript vocabulary below (case-insensitive matching happens downstream; preserve the casing as it appears in the sample).\n- `find` must be a minimal unique string; never whole sentences unless the user asked to reword them.\n- One pair per distinct replacement. If the request is ambiguous, add the most likely interpretation and explain briefly in `note`.\n- If the request cannot be mapped to find/replace operations (e.g. it asks to delete content or is not a correction), return an empty `replacements` array and explain in `note`.\nReply with STRICT JSON only: {{\"replacements\":[{{\"find\":\"...\",\"replace\":\"...\"}}],\"note\":\"...\"}}\n\n=== TRANSCRIPT VOCABULARY (words that appear, with casing preserved) ===\n{word_list}\n\n=== TRANSCRIPT SAMPLE (first 200 segments) ===\n{sample}"
    );
    let request = bea_core::LlmRequest {
        model: effective_meeting_model(&database, &meeting_id, &provider.model),
        system,
        user: instruction.trim().to_string(),
        json_schema: String::new(),
        max_output_tokens: 900,
        reasoning_effort: effective_meeting_reasoning(
            &database,
            &meeting_id,
            &provider.reasoning_effort,
        ),
    };
    let raw = bea_core::call_provider_text(&provider, &request, Some(&api_key))
        .await
        .map_err(command_error)?;
    let parsed: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|error| format!("correction plan was not valid JSON: {error}"))?;
    let mut replacements = Vec::new();
    if let Some(items) = parsed.get("replacements").and_then(|value| value.as_array()) {
        for item in items {
            let Some(find) = item.get("find").and_then(|value| value.as_str()) else {
                continue;
            };
            let Some(replace) = item.get("replace").and_then(|value| value.as_str()) else {
                continue;
            };
            if find.trim().is_empty() || find == replace {
                continue;
            }
            replacements.push(CorrectionPair {
                find: find.to_string(),
                replace: replace.to_string(),
            });
        }
    }
    let note = parsed
        .get("note")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string();
    Ok(CorrectionPlan {
        replacements,
        note,
    })
}

#[derive(serde::Serialize, Clone)]
struct ClarificationSuggestion {
    question: String,
    options: Vec<String>,
}

#[tauri::command]
async fn suggest_clarifications_command(
    state: State<'_, AppState>,
    meeting_id: String,
) -> Result<Vec<ClarificationSuggestion>, String> {
    let database = open_database(&state.database_path).map_err(command_error)?;
    let provider = load_provider(&database, "primary")
        .map_err(command_error)?
        .filter(|provider| provider.enabled);
    let provider =
        provider.ok_or_else(|| "a verified provider is required for clarifications".to_string())?;
    let api_key = if provider.kind == bea_core::ProviderKind::OpenAiOAuth {
        fresh_codex_access_token().await?
    } else {
        keyring_secret(&provider).ok_or_else(|| "provider API key is missing".to_string())?
    };
    let transcript = list_transcript(&database, &meeting_id).map_err(command_error)?;
    if transcript.is_empty() {
        return Err("a transcript is required before clarifications can be suggested".into());
    }
    let speaker_names = bea_core::list_speaker_names(&database, &meeting_id).unwrap_or_default();
    let meeting_context = bea_core::build_meeting_context(&speaker_names, None);
    // Already-answered clarifications (from /clarify or a previous wizard run)
    // are shown so the model does not re-ask the same things.
    let existing = list_context_events_payload(&database, &meeting_id)?;
    let raw_transcript = transcript
        .iter()
        .filter(|segment| segment.text.trim() != "[silence]")
        .map(|segment| {
            let who = segment
                .speaker
                .and_then(|index| speaker_names.get(&index))
                .map(|name| name.as_str())
                .unwrap_or("Speaker ?");
            format!(
                "[{:02}:{:02}] {}: {}",
                segment.start_seconds / 60,
                segment.start_seconds % 60,
                who,
                segment.text.trim()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let system = format!(
        "You review a meeting transcript before formal minutes are written.\n{meeting_context}\nIdentify up to 3 genuine ambiguities that would change the minutes: an unclear decision, an unattributed action item, a vague number/date, or a contradiction. Do NOT ask about procedural noise (greetings, roll call). Each question may have up to 4 candidate answers drawn from the transcript (speaker names, dates, quantities) — leave options empty if free-form input is more natural. Already-recorded notes below must NOT be asked about again.\nReply with STRICT JSON only: {{\"questions\":[{{\"question\":\"...\",\"options\":[\"...\",\"...\"]}}]}}. If nothing is ambiguous, reply {{\"questions\":[]}}.\n\n=== MEETING NOTES ALREADY RECORDED ===\n{existing}\n\n=== TRANSCRIPT ===\n{raw_transcript}"
    );
    let request = bea_core::LlmRequest {
        model: provider.model.clone(),
        system,
        user: "List the clarifying questions.".into(),
        json_schema: String::new(),
        max_output_tokens: 900,
        reasoning_effort: bea_core::normalize_reasoning_effort(&provider.reasoning_effort)
            .to_string(),
    };
    let raw = bea_core::call_provider_text(&provider, &request, Some(&api_key))
        .await
        .map_err(command_error)?;
    // Parse on the Rust side and return typed suggestions; the same
    // parseClarificationSuggestions guard also exists in the UI for defense.
    let parsed: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|error| format!("clarification suggestions were not valid JSON: {error}"))?;
    let mut suggestions = Vec::new();
    if let Some(questions) = parsed.get("questions").and_then(|value| value.as_array()) {
        for question in questions {
            let Some(text) = question.get("question").and_then(|value| value.as_str()) else {
                continue;
            };
            suggestions.push(ClarificationSuggestion {
                question: text.to_string(),
                options: question
                    .get("options")
                    .and_then(|value| value.as_array())
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(|value| value.as_str().map(str::to_string))
                            .take(4)
                            .collect()
                    })
                    .unwrap_or_default(),
            });
        }
    }
    Ok(suggestions)
}

#[tauri::command]
fn save_custom_minutes_format_command(
    state: State<'_, AppState>,
    meeting_id: String,
    format: String,
) -> Result<(), String> {
    let database = open_database(&state.database_path).map_err(command_error)?;
    bea_core::set_app_setting(
        &database,
        &format!("custom_minutes_format:{meeting_id}"),
        format.trim(),
    )
    .map_err(command_error)
}

#[tauri::command]
fn load_custom_minutes_format_command(
    state: State<'_, AppState>,
    meeting_id: String,
) -> Result<String, String> {
    let database = open_database(&state.database_path).map_err(command_error)?;
    Ok(
        bea_core::get_app_setting(&database, &format!("custom_minutes_format:{meeting_id}"))
            .map_err(command_error)?
            .unwrap_or_default(),
    )
}

#[tauri::command]
fn assign_segment_speaker_command(
    state: State<'_, AppState>,
    segment_id: String,
    speaker_index: u32,
) -> Result<(), String> {
    let database = open_database(&state.database_path).map_err(command_error)?;
    bea_core::update_segment_speaker(&database, &segment_id, Some(speaker_index))
        .map_err(command_error)
}

#[tauri::command]
fn set_segment_speakers_command(
    state: State<'_, AppState>,
    segment_id: String,
    speaker_indexes: Vec<u32>,
) -> Result<(), String> {
    let database = open_database(&state.database_path).map_err(command_error)?;
    // An empty selection means "no speaker at all": clear the primary label
    // too, not just the overlap rows.
    if speaker_indexes.is_empty() {
        bea_core::update_segment_speaker(&database, &segment_id, None).map_err(command_error)?;
    }
    bea_core::set_segment_speakers(&database, &segment_id, &speaker_indexes).map_err(command_error)
}

#[tauri::command]
fn list_segment_speakers_command(
    state: State<'_, AppState>,
    meeting_id: String,
) -> Result<std::collections::HashMap<String, Vec<u32>>, String> {
    let database = open_database(&state.database_path).map_err(command_error)?;
    bea_core::list_segment_speakers(&database, &meeting_id).map_err(command_error)
}

#[tauri::command]
fn import_vtt_command(
    state: State<'_, AppState>,
    meeting_id: String,
    path: String,
    title: Option<String>,
) -> Result<Vec<TranscriptSegment>, String> {
    let database = open_database(&state.database_path).map_err(command_error)?;
    let raw = std::fs::read_to_string(&path)
        .map_err(|error| format!("unable to read VTT file: {error}"))?;
    let cues = bea_core::vtt::parse_vtt(&raw).map_err(command_error)?;
    if cues.is_empty() {
        return Err("the VTT file contains no transcript cues".into());
    }
    // Map distinct speaker names to stable indices 0..n, in first-appearance order.
    let mut speaker_index: std::collections::HashMap<String, u32> =
        std::collections::HashMap::new();
    let mut ordered_names: Vec<String> = Vec::new();
    for cue in &cues {
        if let Some(name) = &cue.speaker {
            if !speaker_index.contains_key(name) {
                speaker_index.insert(name.clone(), ordered_names.len() as u32);
                ordered_names.push(name.clone());
            }
        }
    }
    bea_core::clear_transcript(&database, &meeting_id).map_err(command_error)?;
    let mut segments = Vec::new();
    for cue in cues.iter() {
        let segment = TranscriptSegment {
            id: Uuid::new_v4().to_string(),
            meeting_id: meeting_id.clone(),
            start_seconds: cue.start_seconds,
            end_seconds: cue.end_seconds.max(cue.start_seconds),
            text: cue.text.clone(),
            language_detected: None,
            language_confidence: None,
            speaker: cue.speaker.as_ref().map(|name| speaker_index[name]),
        };
        bea_core::add_segment(&database, &segment).map_err(command_error)?;
        segments.push(segment);
    }
    for (index, name) in ordered_names.iter().enumerate() {
        bea_core::set_speaker_name(&database, &meeting_id, index as u32, name)
            .map_err(command_error)?;
    }
    if let Some(new_title) = title.as_deref().filter(|value| !value.trim().is_empty()) {
        update_meeting_title(&database, &meeting_id, new_title).map_err(command_error)?;
    }
    bea_core::set_meeting_status(
        &database,
        &meeting_id,
        bea_core::MeetingStatus::Ready,
        segments
            .last()
            .map(|segment| segment.end_seconds)
            .unwrap_or(0),
    )
    .map_err(command_error)?;
    Ok(segments)
}

#[tauri::command]
fn import_media_command(
    state: State<'_, AppState>,
    meeting_id: String,
    path: String,
    kind: String,
    copy_into_library: bool,
) -> Result<MediaSource, String> {
    let database = open_database(&state.database_path).map_err(command_error)?;
    let media_kind = match kind.as_str() {
        "audio" => MediaKind::Audio,
        "video" => MediaKind::Video,
        _ => return Err("media kind must be audio or video".to_string()),
    };
    let library_root = state
        .database_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    import_media(
        &database,
        &meeting_id,
        path,
        media_kind,
        None,
        copy_into_library,
        library_root,
    )
    .map_err(command_error)
}

#[tauri::command]
async fn process_imported_media_command(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    meeting_id: String,
    path: String,
    kind: String,
    language: String,
) -> Result<Vec<TranscriptSegment>, String> {
    let media_kind = match kind.as_str() {
        "audio" => MediaKind::Audio,
        "video" => MediaKind::Video,
        _ => return Err("media kind must be audio or video".to_string()),
    };
    let root = state
        .database_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let model_root = root.join("models");
    let engine_id = list_meetings(&open_database(&state.database_path).map_err(command_error)?)
        .map_err(command_error)?
        .into_iter()
        .find(|meeting| meeting.id == meeting_id)
        .map(|meeting| meeting.asr_engine_id)
        .unwrap_or_else(|| "whisper-compatibility".into());
    let executable_dir = std::env::current_exe()
        .ok()
        .and_then(|value| value.parent().map(std::path::Path::to_path_buf));
    let sidecar = |name: &str| {
        executable_dir
            .as_ref()
            .map(|dir| dir.join(format!("{name}.exe")))
            .filter(|value| value.is_file())
            .unwrap_or_else(|| root.join("bin").join(format!("{name}.exe")))
    };
    let pipeline = FfmpegPipeline {
        ffmpeg: sidecar("ffmpeg"),
        ffprobe: sidecar("ffprobe"),
    };
    let output_dir = root.join("derived").join(&meeting_id);
    let database_path = state.database_path.clone();
    // Heavy ffmpeg + sherpa-onnx work runs on a worker thread so the UI stays
    // responsive; per-chunk progress is streamed as events.
    let worker_meeting_id = meeting_id.clone();
    let status_meeting_id = meeting_id.clone();
    // Speaker models download once on first use (~35 MB total) so multi-speaker
    // labeling works out of the box. Failure only costs the speaker labels;
    // transcription continues unlabeled.
    if let Err(error) = ensure_speaker_models(&state, &app).await {
        eprintln!("speaker-diarization models unavailable: {error}");
    }
    let worker =
        tauri::async_runtime::spawn_blocking(move || -> Result<Vec<TranscriptSegment>, String> {
            let database = open_database(&database_path).map_err(command_error)?;
            let engine = load_asr_engine(&model_root, &engine_id).map_err(command_error)?;
            // Diarize the whole normalized meeting once (if the speaker models are
            // present) so every chunk can be attributed to a speaker. Read through
            // the resampling helper because normalized audio may not be 16 kHz.
            // Failures here degrade gracefully to unlabeled speakers.
            let audio_path = std::path::Path::new(&output_dir).join("audio.wav");
            let turns = bea_core::read_wav_samples_resampled(&audio_path, 16_000)
                .map(|(samples, _)| samples)
                .and_then(|samples| {
                    bea_core::SpeakerDiarizer::from_models_dir(&model_root)
                        .and_then(|diarizer| diarizer.process_wave(&samples))
                })
                .unwrap_or_default();
            // Re-transcription replaces the previous transcript instead of
            // appending duplicate segments beside it.
            bea_core::clear_transcript(&database, &worker_meeting_id).map_err(command_error)?;
            transcribe_imported_media_with_progress(
                &database,
                &worker_meeting_id,
                std::path::Path::new(&path),
                &media_kind,
                &output_dir,
                &language_from_code(&language),
                &pipeline,
                engine,
                |segment, completed, total| {
                    // Attribute the segment to the speaker active at its midpoint.
                    let midpoint = (segment.start_seconds + segment.end_seconds) as f32 / 2.0;
                    let mut labeled = segment.clone();
                    labeled.speaker = turns
                        .iter()
                        .find(|turn| midpoint >= turn.start && midpoint < turn.end)
                        .map(|turn| turn.speaker);
                    let _ = app.emit(
                        "transcription-progress",
                        serde_json::json!({
                            "meeting_id": meeting_id,
                            "completed": completed,
                            "total": total,
                            "segment": labeled,
                        }),
                    );
                },
            )
            .map(|segments| {
                // Persist speaker labels on the stored segments (the transcribe
                // loop already wrote them without labels).
                segments
                    .into_iter()
                    .map(|mut segment| {
                        let midpoint = (segment.start_seconds + segment.end_seconds) as f32 / 2.0;
                        segment.speaker = turns
                            .iter()
                            .find(|turn| midpoint >= turn.start && midpoint < turn.end)
                            .map(|turn| turn.speaker);
                        let _ = bea_core::update_segment_speaker(
                            &database,
                            &segment.id,
                            segment.speaker,
                        );
                        segment
                    })
                    .collect()
            })
            .map_err(command_error)
        })
        .await;
    let segments = match worker {
        Ok(Ok(segments)) => segments,
        Ok(Err(error)) => {
            record_transcription_failure(&state.database_path, &status_meeting_id, &error)?;
            return Err(error);
        }
        Err(error) => {
            let error = format!("transcription worker failed: {error}");
            record_transcription_failure(&state.database_path, &status_meeting_id, &error)?;
            return Err(error);
        }
    };
    let database = open_database(&state.database_path).map_err(command_error)?;
    bea_core::set_meeting_status(
        &database,
        &status_meeting_id,
        bea_core::MeetingStatus::Ready,
        segments
            .last()
            .map(|segment| segment.end_seconds)
            .unwrap_or(0),
    )
    .map_err(command_error)?;
    bea_core::clear_meeting_last_error(&database, &status_meeting_id).map_err(command_error)?;
    Ok(segments)
}

#[tauri::command]
fn export_minutes_command(
    state: State<'_, AppState>,
    meeting_id: String,
    format: String,
    destination: String,
) -> Result<(), String> {
    let database = open_database(&state.database_path).map_err(command_error)?;
    let minutes = load_minutes(&database, &meeting_id)
        .map_err(command_error)?
        .ok_or_else(|| "minutes have not been saved for this meeting".to_string())?;
    let output_format = match format.as_str() {
        "markdown" => ExportFormat::Markdown,
        "pdf" => ExportFormat::Pdf,
        "docx" => ExportFormat::Docx,
        _ => return Err("format must be markdown, pdf, or docx".to_string()),
    };
    let bytes = export_minutes(&minutes, output_format.clone()).map_err(command_error)?;
    let destination_path = std::path::Path::new(&destination);
    // Refuse suspicious destinations: only document extensions, no overwriting
    // existing files (the UI's save dialog already picks a fresh path).
    let extension = destination_path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());
    let expected = match output_format {
        ExportFormat::Markdown => "md",
        ExportFormat::Pdf => "pdf",
        ExportFormat::Docx => "docx",
    };
    if extension.as_deref() != Some(expected) {
        return Err(format!(
            "export destination must have a .{expected} extension"
        ));
    }
    if destination_path.exists() {
        return Err("export destination already exists; pick a new file name".into());
    }
    if let Some(parent) = destination_path.parent() {
        std::fs::create_dir_all(parent).map_err(command_error)?;
    }
    std::fs::write(destination_path, bytes).map_err(command_error)
}

#[tauri::command]
fn inspect_runtime_command(state: State<'_, AppState>) -> Result<RuntimeAvailability, String> {
    let root = state
        .database_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let model_path = root.join("models");
    // Tauri external binaries are installed beside the application executable;
    // keep the app-data `bin` location as a development/runtime fallback.
    let ffmpeg_path = locate_ffmpeg(root);
    let ocr_path = locate_tesseract(root);
    Ok(inspect_runtime(model_path, ffmpeg_path, ocr_path, None))
}

#[derive(serde::Serialize)]
struct SetupSnapshot {
    runtime: RuntimeAvailability,
    selected_engine: String,
    setup_complete: bool,
    provider_verified: bool,
}

#[tauri::command]
fn setup_status_command(state: State<'_, AppState>) -> Result<SetupSnapshot, String> {
    let root = state
        .database_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let database = open_database(&state.database_path).map_err(command_error)?;
    let provider = load_provider(&database, "primary").map_err(command_error)?;
    let selected_engine = get_app_setting(&database, "selected_asr_engine")
        .map_err(command_error)?
        .unwrap_or_else(|| "whisper-compatibility".into());
    let setup_complete = get_app_setting(&database, "setup_complete")
        .map_err(command_error)?
        .as_deref()
        == Some("true");
    let runtime = inspect_runtime(
        root.join("models"),
        locate_ffmpeg(root),
        locate_tesseract(root),
        provider.as_ref(),
    );
    let provider_verified = provider
        .as_ref()
        .map(|value| {
            value.enabled
                && if value.kind == bea_core::ProviderKind::OpenAiOAuth {
                    load_codex_tokens().is_some()
                } else {
                    keyring_secret(value).is_some()
                }
        })
        .unwrap_or(false);
    Ok(SetupSnapshot {
        runtime,
        selected_engine,
        setup_complete,
        provider_verified,
    })
}

#[tauri::command]
fn set_engine_selection_command(
    state: State<'_, AppState>,
    engine_id: String,
) -> Result<(), String> {
    if !matches!(
        engine_id.as_str(),
        "qwen-standard" | "whisper-compatibility" | "nemotron-multilingual"
    ) {
        return Err("unsupported ASR engine".into());
    }
    let database = open_database(&state.database_path).map_err(command_error)?;
    set_app_setting(&database, "selected_asr_engine", &engine_id).map_err(command_error)
}

#[tauri::command]
fn complete_setup_command(state: State<'_, AppState>) -> Result<(), String> {
    let database = open_database(&state.database_path).map_err(command_error)?;
    set_app_setting(&database, "setup_complete", "true").map_err(command_error)
}

#[tauri::command]
fn list_model_catalog_command(state: State<'_, AppState>) -> Result<Vec<ModelManifest>, String> {
    let database = open_database(&state.database_path).map_err(command_error)?;
    let mut statement = database.prepare("SELECT id,name,version,size_bytes,sha256,runtime,languages,installed FROM model_manifests ORDER BY name").map_err(command_error)?;
    let rows = statement
        .query_map([], |row| {
            let languages =
                serde_json::from_str::<Vec<String>>(&row.get::<_, String>(6)?).unwrap_or_default();
            Ok(ModelManifest {
                id: row.get(0)?,
                name: row.get(1)?,
                version: row.get(2)?,
                size_bytes: row.get(3)?,
                sha256: row.get(4)?,
                runtime: row.get(5)?,
                languages,
                installed: row.get::<_, i64>(7)? != 0,
            })
        })
        .map_err(command_error)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(command_error)
}

#[tauri::command]
fn remove_model_command(state: State<'_, AppState>, engine_id: String) -> Result<(), String> {
    let model_id = match engine_id.as_str() {
        "qwen-standard" => "qwen3-asr-0.6b-int8",
        "whisper-compatibility" => "whisper-compatibility",
        "nemotron-multilingual" => "nemotron-multilingual",
        _ => return Err("unsupported ASR engine".into()),
    };
    let root = state
        .database_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let target = root.join("models").join(model_id);
    if target.exists() {
        std::fs::remove_dir_all(&target).map_err(command_error)?;
    }
    let database = open_database(&state.database_path).map_err(command_error)?;
    database
        .execute(
            "DELETE FROM model_manifests WHERE id=?1",
            rusqlite::params![model_id],
        )
        .map_err(command_error)?;
    Ok(())
}

#[tauri::command]
async fn discover_provider_models_command(
    provider: ProviderConfig,
    api_key: String,
) -> Result<Vec<String>, String> {
    if provider.base_url.trim().is_empty()
        || (api_key.trim().is_empty() && provider.kind.requires_api_key())
    {
        return Err("provider URL and API key are required".into());
    }
    let mut request_builder = http_client().get(format!(
        "{}/models",
        provider.base_url.trim_end_matches('/')
    ));
    // Local servers publish /v1/models without auth.
    if !api_key.trim().is_empty() {
        request_builder = request_builder.bearer_auth(api_key);
    }
    let response = request_builder
        .send()
        .await
        .map_err(command_error)?
        .error_for_status()
        .map_err(command_error)?;
    let payload = response
        .json::<serde_json::Value>()
        .await
        .map_err(command_error)?;
    Ok(payload
        .get("data")
        .and_then(|value| value.as_array())
        .map(|models| {
            models
                .iter()
                .filter_map(|model| {
                    model
                        .get("id")
                        .and_then(|id| id.as_str())
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default())
}

#[derive(serde::Serialize, Clone)]
struct OpenRouterModelInfo {
    id: String,
    name: String,
    context_length: Option<u64>,
    /// True when OpenRouter reports "image" in the model's input modalities.
    vision_capable: bool,
    /// True when OpenRouter reports "audio" in the model's input modalities.
    audio_capable: bool,
    /// True when OpenRouter reports "file" in the model's input modalities.
    file_capable: bool,
}

fn openrouter_input_modalities(
    model: &serde_json::Value,
) -> (bool, bool, bool) {
    // (vision, audio, file) — derived from OpenRouter's architecture
    // input_modalities array on each model entry.
    let modalities = model
        .get("architecture")
        .and_then(|value| value.get("input_modalities"))
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let has = |name: &str| modalities.iter().any(|value| value == &name);
    (has("image"), has("audio"), has("file"))
}

#[tauri::command]
async fn fetch_openrouter_models_command() -> Result<Vec<OpenRouterModelInfo>, String> {
    let response = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(command_error)?
        .get("https://openrouter.ai/api/v1/models")
        .send()
        .await
        .map_err(command_error)?
        .error_for_status()
        .map_err(command_error)?;
    let payload = response
        .json::<serde_json::Value>()
        .await
        .map_err(command_error)?;
    Ok(payload
        .get("data")
        .and_then(|value| value.as_array())
        .map(|models| {
            models
                .iter()
                .filter_map(|model| {
                    Some(OpenRouterModelInfo {
                        id: model.get("id")?.as_str()?.to_string(),
                        name: model
                            .get("name")
                            .and_then(|value| value.as_str())
                            .unwrap_or("")
                            .to_string(),
                        context_length: model
                            .get("context_length")
                            .and_then(|value| value.as_u64()),
                        vision_capable: {
                            let (vision, _, _) = openrouter_input_modalities(model);
                            vision
                        },
                        audio_capable: {
                            let (_, audio, _) = openrouter_input_modalities(model);
                            audio
                        },
                        file_capable: {
                            let (_, _, file) = openrouter_input_modalities(model);
                            file
                        },
                    })
                })
                .collect()
        })
        .unwrap_or_default())
}

#[tauri::command]
fn set_meeting_vision_flag_command(
    state: State<'_, AppState>,
    meeting_id: String,
    vision_capable: bool,
) -> Result<(), String> {
    let database = open_database(&state.database_path).map_err(command_error)?;
    bea_core::set_app_setting(
        &database,
        &format!("vision_capable:{meeting_id}"),
        if vision_capable { "true" } else { "false" },
    )
    .map_err(command_error)
}

/// Decides whether the provider model can see images for this meeting. An
/// explicit per-meeting override wins; otherwise the model id is matched
/// against a small static list of known vision families. Local servers are
/// treated as vision-capable only when the model id carries a vision marker.
fn meeting_vision_capable(
    database: &rusqlite::Connection,
    meeting_id: &str,
    provider_kind: &bea_core::ProviderKind,
    provider_model: &str,
) -> bool {
    if let Ok(Some(flag)) =
        bea_core::get_app_setting(database, &format!("vision_capable:{meeting_id}"))
    {
        return flag == "true";
    }
    let model = provider_model.to_ascii_lowercase();
    if matches!(provider_kind, bea_core::ProviderKind::Local) {
        return ["vl", "vision", "llava", "minicpm-v", "gemma-3"]
            .iter()
            .any(|marker| model.contains(marker));
    }
    if matches!(provider_kind, bea_core::ProviderKind::OpenAiOAuth) {
        return ["gpt-4o", "gpt-4.1", "gpt-5", "o3", "o4"]
            .iter()
            .any(|marker| model.contains(marker));
    }
    [
        "gpt-4o",
        "gpt-4.1",
        "claude-3",
        "claude-4",
        "gemini",
        "pixtral",
        "llama-3.2",
        "qwen2.5-vl",
        "qwen3-vl",
        "vl",
    ]
    .iter()
    .any(|marker| model.contains(marker))
}

/// Resolves the model a meeting's LLM calls should use: the per-meeting
/// override when set (chat tab model picker), otherwise the provider default
/// from Settings.
fn effective_meeting_model(
    database: &rusqlite::Connection,
    meeting_id: &str,
    provider_model: &str,
) -> String {
    bea_core::get_app_setting(database, &format!("meeting_model:{meeting_id}"))
        .ok()
        .flatten()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| provider_model.to_string())
}

#[tauri::command]
fn set_meeting_model_command(
    state: State<'_, AppState>,
    meeting_id: String,
    model: String,
) -> Result<(), String> {
    let database = open_database(&state.database_path).map_err(command_error)?;
    bea_core::set_app_setting(
        &database,
        &format!("meeting_model:{meeting_id}"),
        model.trim(),
    )
    .map_err(command_error)
}

/// Returns the meeting's model override; empty string means "use the provider
/// default".
#[tauri::command]
fn get_meeting_model_command(
    state: State<'_, AppState>,
    meeting_id: String,
) -> Result<String, String> {
    let database = open_database(&state.database_path).map_err(command_error)?;
    Ok(bea_core::get_app_setting(
        &database,
        &format!("meeting_model:{meeting_id}"),
    )
    .map_err(command_error)?
    .unwrap_or_default())
}

/// Resolves the reasoning effort a meeting's LLM calls should use: the
/// per-meeting override when set (chat tab reasoning picker), otherwise the
/// provider default from Settings. Mirrors `effective_meeting_model`.
fn effective_meeting_reasoning(
    database: &rusqlite::Connection,
    meeting_id: &str,
    provider_default: &str,
) -> String {
    let meeting_override =
        bea_core::get_app_setting(database, &format!("meeting_reasoning:{meeting_id}"))
            .ok()
            .flatten();
    bea_core::effective_reasoning_effort(meeting_override.as_deref(), provider_default)
}

#[tauri::command]
fn set_meeting_reasoning_command(
    state: State<'_, AppState>,
    meeting_id: String,
    reasoning: String,
) -> Result<(), String> {
    let database = open_database(&state.database_path).map_err(command_error)?;
    // Empty (the Chat tab's "Default") clears the override so the provider
    // default from Settings applies again; levels are normalized so a typo
    // can never leak into a request body.
    let effort = if reasoning.trim().is_empty() {
        String::new()
    } else {
        bea_core::normalize_reasoning_effort(&reasoning).to_string()
    };
    bea_core::set_app_setting(
        &database,
        &format!("meeting_reasoning:{meeting_id}"),
        &effort,
    )
    .map_err(command_error)
}

/// Returns the meeting's reasoning override; empty string means "use the
/// provider default".
#[tauri::command]
fn get_meeting_reasoning_command(
    state: State<'_, AppState>,
    meeting_id: String,
) -> Result<String, String> {
    let database = open_database(&state.database_path).map_err(command_error)?;
    Ok(bea_core::get_app_setting(
        &database,
        &format!("meeting_reasoning:{meeting_id}"),
    )
    .map_err(command_error)?
    .unwrap_or_default())
}

#[tauri::command]
fn inspect_model_package_command(path: String) -> Result<ModelManifest, String> {
    inspect_asr_model_package(path).map_err(command_error)
}

#[tauri::command]
fn install_model_command(
    state: State<'_, AppState>,
    source: String,
    mut manifest: ModelManifest,
) -> Result<ModelInstallProgress, String> {
    let root = state
        .database_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let destination = root.join("models").join(&manifest.id);
    let progress = if manifest.id == "qwen3-asr-0.6b-int8" {
        install_qwen_model_package(source, &destination, &manifest).map_err(command_error)?
    } else {
        install_sherpa_model_package(source, &destination, &manifest).map_err(command_error)?
    };
    let database = open_database(&state.database_path).map_err(command_error)?;
    manifest.installed = true;
    register_model(&database, &manifest).map_err(command_error)?;
    Ok(progress)
}

#[tauri::command]
async fn download_model_command(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    url: String,
    manifest: ModelManifest,
) -> Result<ModelInstallProgress, String> {
    // The manifest digest is caller-supplied, so it cannot be trusted as
    // verification on its own — but refusing empty/short digests and non-HTTPS
    // URLs closes the trivially-abusable cases (self-asserted "no check", or
    // plaintext download channels).
    if manifest.sha256.trim().len() != 64 {
        return Err("model package must carry a 64-character SHA-256 digest".to_string());
    }
    if !url.starts_with("https://") {
        return Err("model downloads must use HTTPS".to_string());
    }
    let root = state
        .database_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let models = root.join("models");
    std::fs::create_dir_all(&models).map_err(command_error)?;
    let temporary = models.join(format!("{}.download", manifest.id));
    let response = http_client()
        .get(url)
        .send()
        .await
        .map_err(command_error)?
        .error_for_status()
        .map_err(command_error)?;
    let mut output = std::fs::File::create(&temporary).map_err(command_error)?;
    let mut downloaded = 0u64;
    let expected_length = response.content_length();
    let mut response = response;
    while let Some(chunk) = response.chunk().await.map_err(command_error)? {
        output.write_all(&chunk).map_err(command_error)?;
        downloaded += chunk.len() as u64;
        let _ = app.emit(
            "model-install-progress",
            serde_json::json!({
                "model_id": manifest.id.clone(),
                "downloaded": downloaded,
                "total": expected_length,
            }),
        );
    }
    drop(output);
    if let Some(expected_length) = expected_length {
        if expected_length != downloaded {
            let _ = std::fs::remove_file(&temporary);
            return Err(format!(
                "download length mismatch: expected {expected_length}, got {downloaded}"
            ));
        }
    }
    let destination = models.join(&manifest.id);
    let progress = if manifest.id == "qwen3-asr-0.6b-int8" {
        install_qwen_model_package(&temporary, &destination, &manifest).map_err(command_error)?
    } else {
        install_sherpa_model_package(&temporary, &destination, &manifest).map_err(command_error)?
    };
    let _ = std::fs::remove_file(&temporary);
    let database = open_database(&state.database_path).map_err(command_error)?;
    let mut installed_manifest = manifest;
    installed_manifest.installed = true;
    register_model(&database, &installed_manifest).map_err(command_error)?;
    Ok(progress)
}

/// Pinned download sources for every setup dependency. Each engine points at a
/// specific sherpa-onnx release asset so installs are reproducible; the tools
/// point at their official Windows distributions. `sha256` may be empty for
/// moving targets (latest-release builds) — the install step still validates
/// the archive layout before an engine is registered.
#[derive(serde::Serialize, Clone)]
struct DependencyDownload {
    id: &'static str,
    kind: &'static str,
    label: &'static str,
    url: &'static str,
    size_label: &'static str,
    sha256: &'static str,
}

const DEPENDENCY_DOWNLOADS: &[DependencyDownload] = &[
    DependencyDownload {
        id: "whisper-compatibility",
        kind: "engine",
        label: "Bea Standard · Whisper Large-v3-Turbo",
        url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-whisper-turbo.tar.bz2",
        size_label: "≈565 MB archive",
        sha256: "b11acbbcd660b44a8e0df33724feb5aaa709cf65668f2823d59f656312544f22",
    },
    DependencyDownload {
        id: "qwen-standard",
        kind: "engine",
        label: "Qwen3-ASR 0.6B INT8",
        url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25.tar.bz2",
        size_label: "≈880 MB archive",
        sha256: "393f8a14e2f5fb96746aaab342997a40641001fbd5bf9592a080a8329178ee96",
    },
    DependencyDownload {
        id: "nemotron-multilingual",
        kind: "engine",
        label: "Nemotron 3.5 streaming 0.6B INT8 · 560ms",
        url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemotron-3.5-asr-streaming-0.6b-560ms-int8-2026-06-11.tar.bz2",
        size_label: "≈475 MB archive",
        sha256: "c6bf5e0df765f9d5b43bc9e0536d4b4b3e7d40bdf5ecf13e45f134c51c05ae3a",
    },
    DependencyDownload {
        id: "ffmpeg",
        kind: "tool",
        label: "FFmpeg + FFprobe (BtbN win64 GPL build)",
        // Pinned to an exact release with the SHA-256 from BtbN's own
        // checksums.sha256 for that tag; `releases/latest` URLs can swap
        // assets underneath any pin at any time.
        url: "https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-08-25-13-06/ffmpeg-N-126264-g007cd1fd43-win64-gpl.zip",
        size_label: "≈163 MB archive",
        sha256: "776c5c1c379bbd8acf1e0c93005c633ea163ad12ef7914c5bfec0db760164b84",
    },
    DependencyDownload {
        id: "tesseract",
        kind: "tool",
        label: "Tesseract OCR 5 (UB Mannheim installer)",
        url: "https://github.com/UB-Mannheim/tesseract/releases/download/v5.4.0.20240606/tesseract-ocr-w64-setup-5.4.0.20240606.exe",
        size_label: "≈50 MB installer",
        sha256: "",
    },
    DependencyDownload {
        id: "speaker-segmentation",
        kind: "model",
        label: "Pyannote speaker segmentation (diarization)",
        url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-segmentation-models/sherpa-onnx-pyannote-segmentation-3-0.tar.bz2",
        size_label: "≈7 MB archive",
        sha256: "",
    },
    DependencyDownload {
        id: "speaker-embedding",
        kind: "model",
        label: "3D-Speaker embedding model (diarization)",
        url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx",
        size_label: "≈28 MB model",
        sha256: "",
    },
];

#[tauri::command]
fn list_dependency_downloads_command() -> Vec<DependencyDownload> {
    DEPENDENCY_DOWNLOADS.to_vec()
}

/// Streams a file from `url` to `destination`, emitting `dependency-progress`
/// events so the UI can render a progress bar.
async fn stream_dependency_download(
    app: &tauri::AppHandle,
    url: &str,
    destination: &std::path::Path,
    label: &str,
) -> Result<u64, String> {
    let response = http_client()
        .get(url)
        .send()
        .await
        .map_err(command_error)?
        .error_for_status()
        .map_err(command_error)?;
    let mut output = std::fs::File::create(destination).map_err(command_error)?;
    let mut downloaded = 0u64;
    let expected_length = response.content_length();
    let mut response = response;
    while let Some(chunk) = response.chunk().await.map_err(command_error)? {
        output.write_all(&chunk).map_err(command_error)?;
        downloaded += chunk.len() as u64;
        let _ = app.emit(
            "dependency-progress",
            serde_json::json!({
                "id": label,
                "downloaded": downloaded,
                "total": expected_length,
            }),
        );
    }
    drop(output);
    if let Some(expected_length) = expected_length {
        if expected_length != downloaded {
            let _ = std::fs::remove_file(destination);
            return Err(format!(
                "download length mismatch: expected {expected_length}, got {downloaded}"
            ));
        }
    }
    Ok(downloaded)
}

/// Downloads and installs one of the pinned ASR engines directly from the
/// sherpa-onnx release catalog. The archive is checksum-verified when the
/// catalog pins a digest, extracted, and only registered after the required
/// sherpa-onnx model files are present.
#[tauri::command]
async fn download_engine_command(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    engine_id: String,
) -> Result<ModelInstallProgress, String> {
    let entry = DEPENDENCY_DOWNLOADS
        .iter()
        .find(|entry| entry.kind == "engine" && entry.id == engine_id)
        .ok_or_else(|| format!("no pinned download for engine {engine_id}"))?;
    let model_id = match engine_id.as_str() {
        "qwen-standard" => "qwen3-asr-0.6b-int8",
        other => other,
    };
    let root = state
        .database_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let models = root.join("models");
    std::fs::create_dir_all(&models).map_err(command_error)?;
    // Keep the archive extension on the staging file so downstream format
    // detection (and any user inspection) sees a bzip2/tar/zip file.
    let temporary = models.join(format!("{model_id}.download.tar.bz2"));
    let downloaded =
        stream_dependency_download(&app, entry.url, &temporary, engine_id.as_str()).await?;
    let manifest = ModelManifest {
        id: model_id.into(),
        name: entry.label.into(),
        version: "pinned-release".into(),
        size_bytes: downloaded,
        sha256: entry.sha256.into(),
        runtime: "sherpa-onnx".into(),
        languages: match engine_id.as_str() {
            "qwen-standard" => vec!["en".into(), "fil".into(), "taglish".into()],
            _ => vec!["multilingual".into()],
        },
        installed: false,
    };
    let destination = models.join(model_id);
    let progress = if model_id == "qwen3-asr-0.6b-int8" {
        install_qwen_model_package(&temporary, &destination, &manifest).map_err(command_error)?
    } else {
        install_sherpa_model_package(&temporary, &destination, &manifest).map_err(command_error)?
    };
    let _ = std::fs::remove_file(&temporary);
    let database = open_database(&state.database_path).map_err(command_error)?;
    let mut installed_manifest = manifest;
    installed_manifest.installed = true;
    register_model(&database, &installed_manifest).map_err(command_error)?;
    Ok(progress)
}

fn extract_zip_archive(
    source: &std::path::Path,
    destination: &std::path::Path,
) -> Result<(), String> {
    let file = std::fs::File::open(source).map_err(command_error)?;
    let mut archive = zip::ZipArchive::new(file).map_err(command_error)?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(command_error)?;
        let Some(name) = entry.enclosed_name() else {
            return Err("tool archive contains an unsafe path".into());
        };
        let target = destination.join(name);
        if entry.is_dir() {
            std::fs::create_dir_all(&target).map_err(command_error)?;
        } else {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(command_error)?;
            }
            let mut output = std::fs::File::create(&target).map_err(command_error)?;
            std::io::copy(&mut entry, &mut output).map_err(command_error)?;
        }
    }
    Ok(())
}

/// Downloads and installs a local tool (FFmpeg/FFprobe or Tesseract). The
/// bundled-copy repair path stays available as a fallback for offline machines
/// with the full installer payload; this command covers machines that were
/// never bundled with the binaries.
/// Ensure the speaker-diarization models are present in `models/`. Downloads
/// them on first use (they are small: ~7 MB + ~28 MB). Returns Ok(()) when the
/// models are available, Err with the last download error otherwise.
async fn ensure_speaker_models(
    state: &State<'_, AppState>,
    app: &tauri::AppHandle,
) -> Result<(), String> {
    // Only one caller at a time may run the check/download/extract sequence;
    // concurrent transcription jobs would otherwise collide on the same
    // staging files and fail with opaque rename errors. Tokio mutex because
    // the download below awaits while holding the guard.
    let _guard = state.speaker_models_lock.lock().await;
    let root = state
        .database_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let models = root.join("models");
    let segmentation_dir = models.join("pyannote-segmentation-3-0");
    let embedding = models.join("3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx");
    if segmentation_dir.join("model.onnx").is_file() && embedding.is_file() {
        return Ok(());
    }
    std::fs::create_dir_all(&models).map_err(command_error)?;
    for id in ["speaker-segmentation", "speaker-embedding"] {
        let entry = DEPENDENCY_DOWNLOADS
            .iter()
            .find(|entry| entry.id == id)
            .ok_or_else(|| format!("missing catalog entry {id}"))?;
        let temporary = models.join(format!("{id}.download"));
        if !temporary.is_file() {
            stream_dependency_download(app, entry.url, &temporary, id).await?;
        }
        if id == "speaker-segmentation" {
            let staging = models.join("pyannote-staging");
            let _ = std::fs::remove_dir_all(&staging);
            extract_archive_auto(&temporary, &staging)?;
            // Find model.onnx wherever it landed in the archive.
            let model = find_file_by_name(&staging, "model.onnx")
                .ok_or_else(|| "pyannote archive has no model.onnx".to_string())?;
            std::fs::create_dir_all(&segmentation_dir).map_err(command_error)?;
            std::fs::rename(&model, segmentation_dir.join("model.onnx")).map_err(command_error)?;
            let _ = std::fs::remove_dir_all(&staging);
        } else {
            std::fs::rename(&temporary, &embedding).map_err(command_error)?;
        }
        let _ = std::fs::remove_file(&temporary);
    }
    Ok(())
}

/// Extract a zip or tar(.bz2) archive, sniffing the format from magic bytes.
fn extract_archive_auto(
    source: &std::path::Path,
    destination: &std::path::Path,
) -> Result<(), String> {
    std::fs::create_dir_all(destination).map_err(command_error)?;
    if bea_core::file_starts_with(source, b"PK\x03\x04") {
        extract_zip_archive(source, destination)
    } else {
        bea_core::extract_model_archive(source, destination).map_err(|e| e.to_string())
    }
}

/// Recursively find a file with `name` under `dir`.
fn find_file_by_name(dir: &std::path::Path, name: &str) -> Option<std::path::PathBuf> {
    let candidate = dir.join(name);
    if candidate.is_file() {
        return Some(candidate);
    }
    std::fs::read_dir(dir).ok()?.flatten().find_map(|entry| {
        let path = entry.path();
        if path.is_dir() {
            find_file_by_name(&path, name)
        } else {
            None
        }
    })
}

#[tauri::command]
async fn download_tool_command(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    tool: String,
) -> Result<RuntimeAvailability, String> {
    let entry = DEPENDENCY_DOWNLOADS
        .iter()
        .find(|entry| entry.kind == "tool" && entry.id == tool)
        .ok_or_else(|| format!("no pinned download for tool {tool}"))?;
    let root = state
        .database_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let bin_root = root.join("bin");
    std::fs::create_dir_all(&bin_root).map_err(command_error)?;
    let temporary = bin_root.join(format!("{tool}.download"));
    stream_dependency_download(&app, entry.url, &temporary, &tool).await?;
    // Integrity gate: when a digest is pinned, verify before anything touches
    // the download. When it is not (moving upstream builds), at minimum require
    // a valid Authenticode signature on executables before running them.
    if entry.sha256.is_empty() {
        bea_core::verify_executable_authenticode(&temporary).map_err(|error| {
            let _ = std::fs::remove_file(&temporary);
            format!(
                "downloaded {tool} has no pinned checksum and failed its \
                 Authenticode signature check: {error}"
            )
        })?;
    } else {
        bea_core::verify_model_checksum(&temporary, entry.sha256).map_err(|error| {
            let _ = std::fs::remove_file(&temporary);
            error.to_string()
        })?;
    }
    let install_result: Result<(), String> = match tool.as_str() {
        "ffmpeg" => install_downloaded_ffmpeg(&temporary, &bin_root),
        "tesseract" => install_downloaded_tesseract(&temporary, &bin_root).map(|_| ()),
        other => Err(format!("unsupported tool {other}")),
    };
    let _ = std::fs::remove_file(&temporary);
    install_result?;
    let ffmpeg_path = locate_ffmpeg(root);
    let ocr_path = locate_tesseract(root);
    Ok(inspect_runtime(
        root.join("models"),
        ffmpeg_path,
        ocr_path,
        None,
    ))
}

/// The BtbN archive extracts as `ffmpeg-master-latest-win64-gpl/bin/*.exe`;
/// copy both executables into the app-data `bin` directory.
fn install_downloaded_ffmpeg(
    archive: &std::path::Path,
    bin_root: &std::path::Path,
) -> Result<(), String> {
    let staging = bin_root.join("ffmpeg-staging");
    let _ = std::fs::remove_dir_all(&staging);
    extract_zip_archive(archive, &staging)?;
    let mut copied = 0;
    if let Ok(entries) = std::fs::read_dir(&staging) {
        for entry in entries.flatten() {
            let candidate = entry.path().join("bin");
            for name in ["ffmpeg.exe", "ffprobe.exe"] {
                let source = candidate.join(name);
                if source.is_file() {
                    std::fs::copy(&source, bin_root.join(name)).map_err(command_error)?;
                    copied += 1;
                }
            }
        }
    }
    let _ = std::fs::remove_dir_all(&staging);
    if copied < 2 {
        return Err(format!(
            "FFmpeg archive did not contain the expected executables (found {copied}/2)"
        ));
    }
    Ok(())
}

/// The UB-Mannheim installer is an Inno Setup executable; run it silently with
/// a destination inside Bea's app-data so no admin rights are needed.
fn install_downloaded_tesseract(
    installer: &std::path::Path,
    bin_root: &std::path::Path,
) -> Result<PathBuf, String> {
    let target = bin_root.join("tesseract");
    let _ = std::fs::remove_dir_all(&target);
    std::fs::create_dir_all(&target).map_err(command_error)?;
    let mut command = std::process::Command::new(installer);
    command.args([
        "/VERYSILENT",
        "/NORESTART",
        "/SUPPRESSMSGBOXES",
        "/NOCANCEL",
        format!("/DIR={}", target.display()).as_str(),
    ]);
    bea_core::hide_console_window(&mut command);
    let status = command.status().map_err(command_error)?;
    if !status.success() {
        return Err(format!("Tesseract installer exited with {status}"));
    }
    if !target.join("tesseract.exe").is_file() {
        return Err(
            "Tesseract installer did not produce tesseract.exe in the Bea data directory".into(),
        );
    }
    Ok(target)
}

#[tauri::command]
fn load_provider_command(
    state: State<'_, AppState>,
    provider_id: String,
) -> Result<Option<ProviderConfig>, String> {
    let database = open_database(&state.database_path).map_err(command_error)?;
    load_provider(&database, &provider_id).map_err(command_error)
}

#[tauri::command]
fn save_provider_command(
    state: State<'_, AppState>,
    provider: ProviderConfig,
) -> Result<(), String> {
    let database = open_database(&state.database_path).map_err(command_error)?;
    save_provider(&database, &provider).map_err(command_error)
}

#[tauri::command]
fn save_provider_secure_command(
    state: State<'_, AppState>,
    mut provider: ProviderConfig,
    api_key: String,
) -> Result<(), String> {
    if api_key.trim().is_empty() {
        return Err("an API key is required".into());
    }
    let entry = Entry::new("bea-provider", &provider.id).map_err(command_error)?;
    entry.set_password(&api_key).map_err(command_error)?;
    provider.credential_ref = Some(format!("keyring:{}", provider.id));
    provider.enabled = true;
    let database = open_database(&state.database_path).map_err(command_error)?;
    save_provider(&database, &provider).map_err(command_error)
}

#[tauri::command]
fn list_audio_input_devices_command() -> Result<Vec<bea_core::AudioInputDevice>, String> {
    list_audio_input_devices().map_err(command_error)
}

/// Live input level (0.0-1.0) of the meeting's active recorder, for the
/// recording UI's meter. Errors as "recording not found" when idle.
#[tauri::command]
fn recording_level_command(
    state: State<'_, AppState>,
    meeting_id: String,
) -> Result<f32, String> {
    let sender = {
        let recorders = state
            .recorders
            .lock()
            .map_err(|_| "recorder state lock poisoned".to_string())?;
        recorders
            .get(&meeting_id)
            .ok_or_else(|| "recording not found".to_string())?
            .clone()
    };
    let (tx, rx) = mpsc::sync_channel(1);
    sender
        .send(RecorderCommand::Level(tx))
        .map_err(command_error)?;
    rx.recv_timeout(std::time::Duration::from_secs(2))
        .map_err(|_| "recording thread stopped or timed out".to_string())
}

#[tauri::command]
fn start_recording_command(
    state: State<'_, AppState>,
    meeting_id: String,
    device_ids: Option<Vec<String>>,
) -> Result<(), String> {
    // Meeting ids reach filesystem paths here (recordings/<id>); sanitize.
    let meeting_id = sanitize_meeting_id(&meeting_id)?;
    let database = open_database(&state.database_path).map_err(command_error)?;
    let root = state
        .database_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("recordings")
        .join(&meeting_id);
    // Duplicate-recording check must come FIRST: spawning a recorder thread
    // and flipping the DB status before discovering the duplicate orphans the
    // existing recorder.
    if state
        .recorders
        .lock()
        .map_err(|_| "recorder state lock poisoned".to_string())?
        .contains_key(&meeting_id)
    {
        return Err("a recording is already active for this meeting".into());
    }
    bea_core::start_recording(&database, &meeting_id, &root).map_err(command_error)?;
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let commands = spawn_recorder(root, ready_tx, device_ids.unwrap_or_default());
    ready_rx
        .recv()
        .map_err(|_| "recording thread stopped during startup".to_string())??;
    bea_core::set_meeting_status(
        &database,
        &meeting_id,
        bea_core::MeetingStatus::Recording,
        0,
    )
    .map_err(command_error)?;
    let mut recorders = state
        .recorders
        .lock()
        .map_err(|_| "recorder state lock poisoned".to_string())?;
    if recorders.contains_key(&meeting_id) {
        // Race guard: two concurrent starts both passed the early check;
        // re-inserting would orphan the first recorder, so the loser fails.
        return Err("a recording is already active for this meeting".into());
    }
    recorders.insert(meeting_id, commands);
    Ok(())
}

/// Sends a command to the meeting's recorder thread and waits for its reply
/// without holding the recorder-state lock across the wait — a wedged recorder
/// thread must not deadlock every other recorder command. Bounded by a timeout
/// so a stuck thread surfaces as an error instead of hanging the UI.
fn recorder_command_with_reply(
    state: &State<'_, AppState>,
    meeting_id: &str,
    kind: &'static str,
) -> Result<(), String> {
    let sender = {
        let recorders = state
            .recorders
            .lock()
            .map_err(|_| "recorder state lock poisoned".to_string())?;
        recorders
            .get(meeting_id)
            .ok_or_else(|| "recording not found".to_string())?
            .clone()
    };
    let (tx, rx) = mpsc::sync_channel(1);
    let command = match kind {
        "pause" => RecorderCommand::Pause(tx),
        _ => RecorderCommand::Resume(tx),
    };
    sender.send(command).map_err(command_error)?;
    rx.recv_timeout(std::time::Duration::from_secs(10))
        .map_err(|_| "recording thread stopped or timed out".to_string())?
}

#[tauri::command]
fn pause_recording_command(state: State<'_, AppState>, meeting_id: String) -> Result<(), String> {
    recorder_command_with_reply(&state, &meeting_id, "pause")
}

#[tauri::command]
fn resume_recording_command(state: State<'_, AppState>, meeting_id: String) -> Result<(), String> {
    recorder_command_with_reply(&state, &meeting_id, "resume")
}

#[tauri::command]
fn stop_recording_command(
    state: State<'_, AppState>,
    meeting_id: String,
) -> Result<Vec<CompletedAudioChunk>, String> {
    let commands = state
        .recorders
        .lock()
        .map_err(|_| "recorder state lock poisoned".to_string())?
        .remove(&meeting_id)
        .ok_or_else(|| "recording not found".to_string())?;
    let (tx, rx) = mpsc::sync_channel(1);
    commands
        .send(RecorderCommand::Stop(tx))
        .map_err(command_error)?;
    let chunks = rx
        .recv()
        .map_err(|_| "recording thread stopped".to_string())??;
    let duration = chunks.last().map(|chunk| chunk.end_seconds).unwrap_or(0);
    let database = open_database(&state.database_path).map_err(command_error)?;
    bea_core::set_meeting_status(
        &database,
        &meeting_id,
        bea_core::MeetingStatus::Processing,
        duration,
    )
    .map_err(command_error)?;
    for chunk in &chunks {
        persist_completed_audio_chunk(&database, &meeting_id, chunk).map_err(command_error)?;
    }
    create_job(&database, &meeting_id, bea_core::JobKind::Transcription).map_err(command_error)?;
    Ok(chunks)
}

#[tauri::command]
async fn transcribe_recording_command(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    meeting_id: String,
    language: String,
) -> Result<Vec<TranscriptSegment>, String> {
    let database_path = state.database_path.clone();
    let model_root = database_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("models");
    let engine_id = {
        let database = open_database(&database_path).map_err(command_error)?;
        list_meetings(&database)
            .map_err(command_error)?
            .into_iter()
            .find(|meeting| meeting.id == meeting_id)
            .map(|meeting| meeting.asr_engine_id)
            .unwrap_or_else(|| "whisper-compatibility".into())
    };
    let chunks = {
        let database = open_database(&database_path).map_err(command_error)?;
        list_completed_recording_chunks(&database, &meeting_id).map_err(command_error)?
    };
    if chunks.is_empty() {
        return Err("no completed recording chunks are available".to_string());
    }
    let inputs = chunks
        .iter()
        .map(|chunk| AudioChunkInput {
            path: chunk.path.clone(),
            start_seconds: chunk.start_seconds,
            end_seconds: chunk.end_seconds,
        })
        .collect::<Vec<_>>();
    // sherpa-onnx inference is CPU-heavy; run it off the main thread and stream
    // per-chunk progress to the UI.
    let worker_meeting_id = meeting_id.clone();
    // Speaker models download once on first use (~35 MB total) so multi-speaker
    // labeling works out of the box for recordings too. Failure only costs the
    // speaker labels; transcription continues unlabeled.
    if let Err(error) = ensure_speaker_models(&state, &app).await {
        eprintln!("speaker-diarization models unavailable: {error}");
    }
    let worker =
        tauri::async_runtime::spawn_blocking(move || -> Result<Vec<TranscriptSegment>, String> {
            let database = open_database(&database_path).map_err(command_error)?;
            let engine = load_asr_engine(&model_root, &engine_id).map_err(command_error)?;
            // Diarize the full recorded meeting once: concatenate every chunk's
            // samples (resampled to 16 kHz — microphone devices commonly run at
            // 44.1/48 kHz and diarization models expect 16 kHz), then attribute
            // each transcript segment to the speaker active at its midpoint.
            // Failures here degrade gracefully to unlabeled speakers.
            // Diarization memory guard: cap the concatenated audio at the
            // first 60 minutes (16 kHz mono f32 ≈ 230 GB-equivalent would
            // otherwise be unbounded for hour-long meetings).
            const DIARIZATION_MAX_SECONDS: f32 = 60.0 * 60.0;
            const DIARIZATION_MAX_SAMPLES: usize =
                (DIARIZATION_MAX_SECONDS * 16_000.0) as usize;
            let mut meeting_samples: Vec<f32> = Vec::new();
            let mut truncated = false;
            'collect: for input in &inputs {
                if let Some((samples, _)) =
                    bea_core::read_wav_samples_resampled(&input.path, 16_000)
                {
                    let remaining = DIARIZATION_MAX_SAMPLES - meeting_samples.len();
                    if samples.len() > remaining {
                        meeting_samples.extend_from_slice(&samples[..remaining]);
                        truncated = true;
                        break 'collect;
                    }
                    meeting_samples.extend(samples);
                }
            }
            if truncated {
                eprintln!(
                    "diarization input truncated to the first {DIARIZATION_MAX_SECONDS:.0} minutes of audio"
                );
            }
            let turns = if meeting_samples.is_empty() {
                Vec::new()
            } else {
                bea_core::SpeakerDiarizer::from_models_dir(&model_root)
                    .and_then(|diarizer| diarizer.process_wave(&meeting_samples))
                    .unwrap_or_default()
            };
            let speaker_at = |segment: &TranscriptSegment| -> Option<u32> {
                let midpoint = (segment.start_seconds + segment.end_seconds) as f32 / 2.0;
                turns
                    .iter()
                    .find(|turn| midpoint >= turn.start && midpoint < turn.end)
                    .map(|turn| turn.speaker)
            };
            // Re-transcription replaces the previous transcript instead of
            // appending duplicate segments beside it.
            bea_core::clear_transcript(&database, &worker_meeting_id).map_err(command_error)?;
            transcribe_chunks_with_progress(
                &database,
                &worker_meeting_id,
                &inputs,
                &language_from_code(&language),
                engine,
                |segment, completed, total| {
                    let mut labeled = segment.clone();
                    labeled.speaker = speaker_at(segment);
                    let _ = app.emit(
                        "transcription-progress",
                        serde_json::json!({
                            "meeting_id": worker_meeting_id,
                            "completed": completed,
                            "total": total,
                            "segment": labeled,
                        }),
                    );
                },
            )
            .map(|segments| {
                // Persist speaker labels on the stored segments (the transcribe
                // loop already wrote them without labels).
                segments
                    .into_iter()
                    .map(|mut segment| {
                        segment.speaker = speaker_at(&segment);
                        let _ = bea_core::update_segment_speaker(
                            &database,
                            &segment.id,
                            segment.speaker,
                        );
                        segment
                    })
                    .collect()
            })
            .map_err(command_error)
        })
        .await;
    let segments = match worker {
        Ok(Ok(segments)) => segments,
        Ok(Err(error)) => {
            record_transcription_failure(&state.database_path, &meeting_id, &error)?;
            return Err(error);
        }
        Err(error) => {
            let error = format!("transcription worker failed: {error}");
            record_transcription_failure(&state.database_path, &meeting_id, &error)?;
            return Err(error);
        }
    };
    let database = open_database(&state.database_path).map_err(command_error)?;
    database
        .execute(
            "UPDATE jobs SET state='completed',progress=1,error=NULL WHERE id=(SELECT id FROM jobs WHERE meeting_id=?1 AND kind='transcription' ORDER BY rowid DESC LIMIT 1)",
            rusqlite::params![meeting_id],
        )
        .map_err(command_error)?;
    let duration = segments
        .last()
        .map(|segment| segment.end_seconds)
        .unwrap_or(0);
    bea_core::set_meeting_status(
        &database,
        &meeting_id,
        bea_core::MeetingStatus::Ready,
        duration,
    )
    .map_err(command_error)?;
    bea_core::clear_meeting_last_error(&database, &meeting_id).map_err(command_error)?;
    Ok(segments)
}

#[tauri::command]
async fn check_for_updates_command(
    app: tauri::AppHandle,
) -> Result<Option<bea_core::updater::UpdateInfo>, String> {
    bea_core::updater::check_now(&app).await
}

#[tauri::command]
async fn install_update_command(app: tauri::AppHandle) -> Result<(), String> {
    bea_core::updater::download_and_install(&app).await
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            app.manage(AppState {
                database_path: data_dir.join("bea.db"),
                recorders: Mutex::new(HashMap::new()),
                speaker_models_lock: tauri::async_runtime::Mutex::new(()),
                cancelled_minutes: Mutex::new(std::collections::HashSet::new()),
            });
            bea_core::updater::spawn_scheduled_checks(app.handle().clone());
            Ok(())
        })
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
        modify_minutes_command,
            create_meeting_command,
            list_meetings_command,
            rename_meeting_command,
            delete_meeting_command,
            delete_all_data_command,
            delete_transcript_segments_command,
            cancel_minutes_generation_command,
            update_transcript_segment_command,
            list_media_command,
            extract_frames_command,
            ensure_playable_proxy_command,
            waveform_peaks_command,
            repair_runtime_command,
            test_provider_connection_command,
            validate_minutes_model_command,
            search_transcript_command,
            list_transcript_command,
            load_minutes_command,
            preview_provider_context_command,
            save_minutes_command,
            generate_minutes_command,
            import_media_command,
            import_vtt_command,
            set_speaker_name_command,
            list_speaker_names_command,
            assign_segment_speaker_command,
            set_segment_speakers_command,
            list_segment_speakers_command,
            add_context_event_command,
            list_context_events_command,
            delete_context_event_command,
            clear_ai_memory_command,
            apply_mass_correction_command,
            chat_command,
            save_custom_minutes_format_command,
            suggest_clarifications_command,
            load_custom_minutes_format_command,
            process_imported_media_command,
            export_minutes_command,
            inspect_runtime_command,
            setup_status_command,
            set_engine_selection_command,
            complete_setup_command,
            list_model_catalog_command,
            remove_model_command,
            discover_provider_models_command,
            fetch_openrouter_models_command,
            set_meeting_vision_flag_command,
            set_meeting_model_command,
            get_meeting_model_command,
            set_meeting_reasoning_command,
            get_meeting_reasoning_command,
            resolve_correction_command,
            inspect_model_package_command,
            install_model_command,
            download_model_command,
            list_dependency_downloads_command,
            download_engine_command,
            download_tool_command,
            load_provider_command,
            save_provider_command,
            save_provider_secure_command,
            codex_oauth_login_command,
            codex_oauth_status_command,
            codex_oauth_sign_out_command,
            codex_list_models_command,
            list_audio_input_devices_command,
            recording_level_command,
            start_recording_command,
            pause_recording_command,
            resume_recording_command,
            stop_recording_command,
            transcribe_recording_command,
            check_for_updates_command,
            install_update_command
        ])
        .run(tauri::generate_context!())
        .expect("error while running Bea");
}

#[cfg(test)]
mod tests {
    use super::{locate_tesseract, minutes_failure_message, parse_oauth_callback, sanitize_meeting_id, verify_media_source};

    #[test]
    fn minutes_failure_guidance_never_doubles_the_period() {
        // Provider errors usually end with a sentence period; bolting the
        // guidance sentence on must not render "fallback.. Fix the provider".
        let message = minutes_failure_message("Your ChatGPT plan's usage limit is reached — it resets in 6 days.");
        assert!(message.ends_with("local heuristic minutes."), "got: {message}");
        assert!(!message.contains(".."), "got: {message}");
        let bare = minutes_failure_message("no punctuation here");
        assert!(bare.contains("no punctuation here. Fix the provider"), "got: {bare}");
    }

    #[test]
    fn tesseract_locator_checks_installed_windows_location() {
        let path = locate_tesseract(std::path::Path::new("missing-bea-data"));
        if let Ok(program_files) = std::env::var("ProgramFiles") {
            let installed = std::path::PathBuf::from(program_files)
                .join("Tesseract-OCR")
                .join("tesseract.exe");
            if installed.is_file() {
                assert_eq!(path, installed);
            }
        }
    }

    #[test]
    fn sanitize_meeting_id_accepts_uuid_style_ids() {
        assert_eq!(
            sanitize_meeting_id("abc-123_XY.9").unwrap(),
            "abc-123_XY.9"
        );
        assert!(sanitize_meeting_id("6f1c2e3a-1b2c-3d4e-5f6a-7b8c9d0e1f2a").is_ok());
    }

    #[test]
    fn sanitize_meeting_id_rejects_traversal_and_separators() {
        assert!(sanitize_meeting_id("").is_err());
        assert!(sanitize_meeting_id("..").is_err());
        assert!(sanitize_meeting_id(".").is_err());
        assert!(sanitize_meeting_id("../../etc").is_err());
        assert!(sanitize_meeting_id("..\\..\\x").is_err());
        assert!(sanitize_meeting_id("a/b").is_err());
        assert!(sanitize_meeting_id("a\\b").is_err());
        assert!(sanitize_meeting_id("a b").is_err());
        assert!(sanitize_meeting_id("a:b").is_err());
    }

    #[test]
    fn parse_oauth_callback_extracts_code_and_validates_state() {
        let line = "GET /auth/callback?code=abc123&state=st-1 HTTP/1.1";
        assert_eq!(parse_oauth_callback(line, "st-1").unwrap(), "abc123");
        // Wrong or missing state must be rejected (CSRF guard).
        assert!(parse_oauth_callback(line, "st-2").is_err());
        assert!(
            parse_oauth_callback("GET /auth/callback?code=abc123 HTTP/1.1", "st-1").is_err()
        );
        // Missing code with valid state errors on the code, not the state.
        let err = parse_oauth_callback(
            "GET /auth/callback?state=st-1 HTTP/1.1",
            "st-1",
        )
        .unwrap_err();
        assert!(err.contains("authorization code"));
        // No query string at all.
        assert!(parse_oauth_callback("GET /auth/callback HTTP/1.1", "st-1").is_err());
        // code-like substrings in other params must not be picked up.
        let tricky =
            "GET /auth/callback?notcode=x&code=real&state=st-1 HTTP/1.1";
        assert_eq!(parse_oauth_callback(tricky, "st-1").unwrap(), "real");
    }

    #[test]
    fn verify_media_source_accepts_registered_path_and_rejects_strangers() {
        let database = rusqlite::Connection::open_in_memory().unwrap();
        database
            .execute_batch(
                "CREATE TABLE media_sources (id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL, path TEXT NOT NULL, kind TEXT NOT NULL, duration_seconds INTEGER, copied INTEGER NOT NULL DEFAULT 0);",
            )
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("meeting.mkv");
        std::fs::write(&media, b"x").unwrap();
        database
            .execute(
                "INSERT INTO media_sources(id,meeting_id,path,kind) VALUES ('m1','meet',?1,'video')",
                rusqlite::params![media.to_string_lossy()],
            )
            .unwrap();
        // Exact registered path passes.
        assert!(verify_media_source(&database, "meet", &media).is_ok());
        // Separator style differences still match (Windows back/forward slash).
        let forward = std::path::PathBuf::from(media.to_string_lossy().replace('\\', "/"));
        assert!(verify_media_source(&database, "meet", &forward).is_ok());
        // Unregistered path is refused.
        let stranger = dir.path().join("stranger.mkv");
        std::fs::write(&stranger, b"x").unwrap();
        assert!(verify_media_source(&database, "meet", &stranger).is_err());
        // Right path, wrong meeting is refused.
        assert!(verify_media_source(&database, "other", &media).is_err());
    }
}
