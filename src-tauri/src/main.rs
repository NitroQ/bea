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
}

enum RecorderCommand {
    Pause(mpsc::SyncSender<Result<(), String>>),
    Resume(mpsc::SyncSender<Result<(), String>>),
    Stop(mpsc::SyncSender<Result<Vec<CompletedAudioChunk>, String>>),
}

const MEETING_SECRETARY_SYSTEM_PROMPT: &str = r#"You are an expert meeting secretary. From the transcript events below, produce strict JSON meeting minutes in English.
Rules:
- summary: a 2-4 sentence executive summary of the whole meeting.
- decisions / action_items / unresolved: exactly one entry per distinct point; merge duplicates. Each entry's summary must be a self-contained, third-person, present-tense statement of the outcome or task (e.g. "The passing grade is set to 2.0 effective AY 2026-2027", "Revise the 2-day loss list before Friday") — never a question, never a verbatim or near-verbatim transcript line, and never in the speaker's voice. Include the owner and deadline in the summary when the transcript names them.
- agenda: the meeting's topics in the order they were discussed, each as a short noun-phrase heading (e.g. "Q3 budget review"); attach start_seconds/end_seconds from the transcript when the topic's discussion span is identifiable. Infer topics from how the conversation shifts even when no written agenda exists — every meeting that discussed distinct subjects has an agenda. Return an empty array only when the whole meeting is a single informal topic.
- Every item's summary must be ONE concise, capitalized, grammatical headline sentence in English (example: "Admission limits remain at the discretion of the Dean"). NEVER copy raw transcript speech as a summary.
- Every item's evidence: list the EXACT original quotes with the start_seconds/end_seconds taken from the matching input event. Do not paraphrase quotes or invent timestamps.
- Exclude procedural noise (motions to approve past minutes, roll call, greetings, filler) from action items and decisions.
- If a Participants legend or custom format is provided in the system prompt, use real participant names instead of "Speaker N" and follow the custom format's structure while still returning the same JSON schema.
- visual_observations: when visual context (frame images or OCR text of video frames) is provided, add one short observation string per notable thing seen on the frames (e.g. "Slide at 320s shows the Q3 budget table"). Omit the array or return [] when no visual context is provided."#;

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

/// Shared HTTP client with a 120 s timeout. Requests without a timeout used to
/// hang the command forever when a provider stalled; the OpenRouter catalog
/// call keeps its own shorter 30 s timeout.
fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
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

fn build_input_stream(
    device: &cpal::Device,
    supported: &cpal::SupportedStreamConfig,
    recorder: Arc<Mutex<SegmentedWavRecorder>>,
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
    match supported.sample_format() {
        cpal::SampleFormat::F32 => device.build_input_stream(
            &config,
            move |data: &[f32], _| {
                if let Ok(mut recorder) = recorder.lock() {
                    let samples: Vec<i16> = data
                        .iter()
                        .map(|sample| (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
                        .collect();
                    let _ = recorder.push_samples(&samples);
                }
            },
            error_callback,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            &config,
            move |data: &[i16], _| {
                if let Ok(mut recorder) = recorder.lock() {
                    let _ = recorder.push_samples(data);
                }
            },
            error_callback,
            None,
        ),
        cpal::SampleFormat::U16 => device.build_input_stream(
            &config,
            move |data: &[u16], _| {
                if let Ok(mut recorder) = recorder.lock() {
                    let samples: Vec<i16> = data
                        .iter()
                        .map(|sample| (*sample as i32 - 32768) as i16)
                        .collect();
                    let _ = recorder.push_samples(&samples);
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
) -> Sender<RecorderCommand> {
    let (commands, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| -> Result<(), String> {
            let device = cpal::default_host()
                .default_input_device()
                .ok_or_else(|| "no default microphone is available".to_string())?;
            let supported = device.default_input_config().map_err(command_error)?;
            let mut recorder = SegmentedWavRecorder::new(
                root,
                RecorderConfig {
                    sample_rate: supported.sample_rate().0,
                    channels: supported.channels(),
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
            let stream =
                build_input_stream(&device, &supported, Arc::clone(&recorder), Arc::clone(&stream_error))?;
            stream.play().map_err(command_error)?;
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
                    RecorderCommand::Stop(reply) => {
                        let result = recorder
                            .lock()
                            .map_err(|_| "recorder lock poisoned".to_string())
                            .and_then(|mut value| value.stop().map_err(command_error));
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
            drop(stream);
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
    Ok(())
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

fn load_codex_tokens() -> Option<bea_core::codex_oauth::CodexTokens> {
    let raw = Entry::new("bea-provider", CODEX_KEYRING_ID)
        .ok()?
        .get_password()
        .ok()?;
    serde_json::from_str(&raw).ok()
}

fn store_codex_tokens(tokens: &bea_core::codex_oauth::CodexTokens) -> Result<(), String> {
    let entry = Entry::new("bea-provider", CODEX_KEYRING_ID).map_err(command_error)?;
    entry
        .set_password(&serde_json::to_string(tokens).map_err(command_error)?)
        .map_err(command_error)
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
    // Open the default browser on the Windows host.
    std::process::Command::new("cmd")
        .args(["/C", "start", "", &url])
        .spawn()
        .map_err(|e| format!("could not open the browser: {e}"))?;
    let code = tauri::async_runtime::spawn_blocking(move || -> Result<String, String> {
        let listener = std::net::TcpListener::bind("127.0.0.1:1455")
            .map_err(|e| format!("port 1455 unavailable (another login is running?): {e}"))?;
        let (mut stream, _) = listener.accept().map_err(|e| e.to_string())?;
        let mut buffer = [0u8; 4096];
        let read = stream.read(&mut buffer).map_err(|e| e.to_string())?;
        let _ = stream.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n<html><body style='font-family:sans-serif'><h2>Bea is connected.</h2>You can close this tab.</body></html>",
        );
        // Request line: GET /auth/callback?code=...&state=... HTTP/1.1
        let request = String::from_utf8_lossy(&buffer[..read]).to_string();
        let line = request.lines().next().unwrap_or_default();
        parse_oauth_callback(line, &state)
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
    let token = fresh_codex_access_token().await?;
    let response = http_client()
        .get(format!(
            "{}/models",
            bea_core::codex_oauth::CHATGPT_API_BASE
        ))
        .bearer_auth(token)
        .send()
        .await
        .map_err(command_error)?;
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
                let request = LlmRequest {
                    model: effective_model,
                    system: format!("{}\n{}", MEETING_SECRETARY_SYSTEM_PROMPT, meeting_context),
                    user: serde_json::to_string(&pack.events).map_err(command_error)?,
                    json_schema: r#"{"type":"object","properties":{"title":{"type":"string"},"summary":{"type":"string"},"agenda":{"type":"array","items":{"type":"object","properties":{"heading":{"type":"string"},"start_seconds":{"type":"number"},"end_seconds":{"type":"number"}},"required":["heading"]}},"visual_observations":{"type":"array","items":{"type":"string"}},"decisions":{"type":"array","items":{"type":"object","properties":{"kind":{"type":"string"},"summary":{"type":"string"},"confidence":{"type":"number"},"evidence":{"type":"array","items":{"type":"object","properties":{"start_seconds":{"type":"number"},"end_seconds":{"type":"number"},"quote":{"type":"string"}},"required":["start_seconds","end_seconds","quote"]}}},"required":["summary","evidence"]}},"action_items":{"type":"array","items":{"type":"object","properties":{"kind":{"type":"string"},"summary":{"type":"string"},"confidence":{"type":"number"},"evidence":{"type":"array","items":{"type":"object","properties":{"start_seconds":{"type":"number"},"end_seconds":{"type":"number"},"quote":{"type":"string"}},"required":["start_seconds","end_seconds","quote"]}}},"required":["summary","evidence"]}},"unresolved":{"type":"array","items":{"type":"object","properties":{"kind":{"type":"string"},"summary":{"type":"string"},"confidence":{"type":"number"},"evidence":{"type":"array","items":{"type":"object","properties":{"start_seconds":{"type":"number"},"end_seconds":{"type":"number"},"quote":{"type":"string"}},"required":["start_seconds","end_seconds","quote"]}}},"required":["summary","evidence"]}}},"required":["title","summary","decisions","action_items","unresolved"]}"#.into(),
                    max_output_tokens: 4_000,
                };
                let remote_result = if vision_images.is_empty() {
                    call_provider(&provider, &request, Some(&api_key)).await
                } else {
                    // Vision path: send the frame images inline and parse the
                    // minutes JSON out of the free-text reply.
                    bea_core::call_provider_messages(
                        &provider,
                        &request.system,
                        &request.user,
                        &vision_images,
                        request.max_output_tokens,
                        Some(&api_key),
                    )
                    .await
                    .and_then(|text| bea_core::parse_minutes_json(&text))
                };
                match remote_result {
                    Ok(remote_minutes) => {
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
                        return Err(format!(
                            "AI minutes failed: {provider_error}. Fix the provider in Settings, or disable it to use local heuristic minutes."
                        ));
                    }
                }
            }
        }
    }
    save_minutes(&database, &meeting_id, &minutes).map_err(command_error)?;
    Ok(minutes)
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

#[tauri::command]
async fn chat_command(
    state: State<'_, AppState>,
    meeting_id: String,
    question: String,
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
    let system = format!(
        "You are Bea, an assistant answering questions about one meeting.\n{meeting_context}\nAnswer using ONLY the transcript, notes, and any visual context below. Cite speaker names and timestamps. If the answer is not in the material, say so plainly.\n\n=== MEETING NOTES (user-added clarifications/context) ===\n{}\n\n=== MEETING LEDGER ===\n{}\n\n=== FULL TRANSCRIPT ===\n{raw_transcript}{ocr_block}",
        list_context_events_payload(&database, &meeting_id)?,
        serde_json::to_string(&pack.events).map_err(command_error)?
    );
    let answer = if vision_images.is_empty() {
        let request = bea_core::LlmRequest {
            model: effective_meeting_model(&database, &meeting_id, &provider.model),
            system,
            user: question.trim().to_string(),
            json_schema: String::new(),
            max_output_tokens: 1_500,
        };
        bea_core::call_provider_text(&provider, &request, Some(&api_key))
            .await
            .map_err(command_error)?
    } else {
        bea_core::call_provider_messages(
            &provider,
            &system,
            question.trim(),
            &vision_images,
            1_500,
            Some(&api_key),
        )
        .await
        .map_err(command_error)?
    };
    // Persist the Q&A pair so it survives restarts and feeds future minutes.
    add_context_event_command_inner(
        &database,
        &meeting_id,
        "chat",
        &format!("Q: {}\nA: {}", question.trim(), answer),
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
    let segments =
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
        .await
        .map_err(|error| format!("transcription worker failed: {error}"))??;
    bea_core::set_meeting_status(
        &open_database(&state.database_path).map_err(command_error)?,
        &status_meeting_id,
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
                        vision_capable: model
                            .get("architecture")
                            .and_then(|value| value.get("input_modalities"))
                            .and_then(|value| value.as_array())
                            .map(|values| {
                                values
                                    .iter()
                                    .any(|value| value.as_str() == Some("image"))
                            })
                            .unwrap_or(false),
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

#[tauri::command]
fn start_recording_command(state: State<'_, AppState>, meeting_id: String) -> Result<(), String> {
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
    let commands = spawn_recorder(root, ready_tx);
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
    let segments =
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
        .await
        .map_err(|error| format!("transcription worker failed: {error}"))??;
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
    Ok(segments)
}

#[tauri::command]
fn check_for_updates_command(
    app: tauri::AppHandle,
) -> Result<Option<bea_core::updater::UpdateInfo>, String> {
    bea_core::updater::check_now(&app)
}

#[tauri::command]
fn install_update_command(app: tauri::AppHandle) -> Result<(), String> {
    bea_core::updater::download_and_install(&app)
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
            update_transcript_segment_command,
            list_media_command,
            extract_frames_command,
            ensure_playable_proxy_command,
            waveform_peaks_command,
            repair_runtime_command,
            test_provider_connection_command,
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
    use super::{locate_tesseract, parse_oauth_callback, sanitize_meeting_id, verify_media_source};

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
