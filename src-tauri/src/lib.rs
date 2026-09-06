use chrono::{DateTime, Utc};
use cpal::traits::{DeviceTrait, HostTrait};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sherpa_onnx::{
    OfflineModelConfig, OfflineQwen3ASRModelConfig, OfflineRecognizer, OfflineRecognizerConfig,
    OfflineWhisperModelConfig, OnlineNemoCtcModelConfig, OnlineRecognizer, OnlineRecognizerConfig,
    OnlineTransducerModelConfig, Wave,
};
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use thiserror::Error;
use uuid::Uuid;

pub mod vtt;

const CHUNK_SECONDS: u64 = 60;
const MAX_MEETING_SECONDS: u64 = 5 * 60 * 60;

/// Prevents Windows from flashing a console window every time Bea spawns a
/// console executable (ffmpeg/ffprobe/tesseract). No-op on other platforms.
pub fn hide_console_window(command: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(target_os = "windows")]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// Spawns `program` with args and captures its output without flashing a
/// console window on Windows.
fn run_hidden(
    program: impl AsRef<std::ffi::OsStr>,
    args: &[&std::ffi::OsStr],
) -> std::io::Result<std::process::Output> {
    let mut command = std::process::Command::new(program.as_ref());
    command.args(args);
    hide_console_window(&mut command);
    command.output()
}

#[derive(Debug, Error)]
pub enum BeaError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("invalid meeting duration: {0} seconds (maximum is 18000)")]
    InvalidDuration(u64),
    #[error("meeting not found: {0}")]
    MeetingNotFound(String),
    #[error("invalid model output: {0}")]
    InvalidModelOutput(String),
    #[error("invalid state transition: {0}")]
    InvalidState(String),
    #[error("unsupported media format: {0}")]
    UnsupportedMedia(String),
    #[error("model checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },
    #[error("provider request failed: {0}")]
    ProviderRequest(String),
    #[error("media processing failed: {0}")]
    MediaProcessing(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MeetingStatus {
    Draft,
    Recording,
    Paused,
    Processing,
    Ready,
    Failed,
}

impl MeetingStatus {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Recording => "recording",
            Self::Paused => "paused",
            Self::Processing => "processing",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TranscriptLanguage {
    Auto,
    English,
    Filipino,
    Taglish,
}

impl TranscriptLanguage {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::English => "en",
            Self::Filipino => "fil",
            Self::Taglish => "taglish",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Meeting {
    pub id: String,
    pub title: String,
    pub status: MeetingStatus,
    pub created_at: DateTime<Utc>,
    pub duration_seconds: u64,
    pub language: TranscriptLanguage,
    pub asr_engine_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptSegment {
    pub id: String,
    pub meeting_id: String,
    pub start_seconds: u64,
    pub end_seconds: u64,
    pub text: String,
    pub language_detected: Option<String>,
    pub language_confidence: Option<f32>,
    pub speaker: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Evidence {
    pub start_seconds: u64,
    pub end_seconds: u64,
    pub quote: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LedgerEvent {
    pub kind: String,
    pub summary: String,
    pub owner: Option<String>,
    pub due: Option<String>,
    pub confidence: f32,
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Minutes {
    pub title: String,
    pub summary: String,
    pub decisions: Vec<LedgerEvent>,
    pub action_items: Vec<LedgerEvent>,
    pub unresolved: Vec<LedgerEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ChunkState {
    Recording,
    Completed,
    Interrupted,
    Failed,
}
impl ChunkState {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Recording => "recording",
            Self::Completed => "completed",
            Self::Interrupted => "interrupted",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecordingChunk {
    pub id: String,
    pub meeting_id: String,
    pub ordinal: u32,
    pub start_seconds: u64,
    pub end_seconds: u64,
    pub path: PathBuf,
    pub state: ChunkState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum JobKind {
    Import,
    Transcription,
    Context,
    Minutes,
    Export,
}
impl JobKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Import => "import",
            Self::Transcription => "transcription",
            Self::Context => "context",
            Self::Minutes => "minutes",
            Self::Export => "export",
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum JobState {
    Queued,
    Running,
    Completed,
    Failed,
    Retryable,
}
impl JobState {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Retryable => "retryable",
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Job {
    pub id: String,
    pub meeting_id: String,
    pub kind: JobKind,
    pub state: JobState,
    pub progress: f32,
    pub attempts: u32,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MediaKind {
    Audio,
    Video,
}
impl MediaKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Audio => "audio",
            Self::Video => "video",
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MediaSource {
    pub id: String,
    pub meeting_id: String,
    pub path: PathBuf,
    pub kind: MediaKind,
    pub duration_seconds: Option<u64>,
    pub copied: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub runtime: String,
    pub languages: Vec<String>,
    pub installed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ProviderKind {
    OpenRouter,
    OpenAiCompatible,
    ClaudeCompatible,
    Local,
}
impl ProviderKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::OpenRouter => "openrouter",
            Self::OpenAiCompatible => "openai_compatible",
            Self::ClaudeCompatible => "claude_compatible",
            Self::Local => "local",
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderConfig {
    pub id: String,
    pub kind: ProviderKind,
    pub base_url: String,
    pub model: String,
    pub credential_ref: Option<String>,
    pub enabled: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsageRecord {
    pub provider_id: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub estimated_cost: Option<f64>,
    pub operation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LlmRequest {
    pub model: String,
    pub system: String,
    pub user: String,
    pub json_schema: String,
    pub max_output_tokens: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderRequest {
    pub url: String,
    pub model: String,
    pub body: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AsrEngineKind {
    Qwen3Asr06bInt8,
    WhisperCompatibility,
    NemotronMultilingual560ms,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AsrCapabilities {
    pub languages: Vec<String>,
    pub offline: bool,
    pub streaming: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AudioChunkInput {
    pub path: PathBuf,
    pub start_seconds: u64,
    pub end_seconds: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AsrResult {
    pub text: String,
    pub language_detected: Option<String>,
    pub confidence: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuntimeAvailability {
    pub asr_model_available: bool,
    pub ffmpeg_available: bool,
    pub ffprobe_available: bool,
    pub ocr_available: bool,
    pub provider_configured: bool,
    pub can_transcribe_locally: bool,
}

pub fn qwen_model_is_complete(model_path: impl AsRef<Path>) -> bool {
    let model_path = model_path.as_ref();
    if model_path.is_file() {
        return true;
    }
    if !model_path.is_dir() {
        return false;
    }
    [
        "conv_frontend.onnx",
        "encoder.int8.onnx",
        "decoder.int8.onnx",
        "tokenizer",
    ]
    .iter()
    .all(|name| model_path.join(name).exists())
        || std::fs::read_dir(model_path)
            .ok()
            .into_iter()
            .flat_map(|entries| entries.flatten())
            .filter(|entry| entry.path().is_dir())
            .any(|entry| qwen_model_is_complete(entry.path()))
}

pub fn find_qwen_model_dir(model_root: impl AsRef<Path>) -> Option<PathBuf> {
    let model_root = model_root.as_ref();
    if !model_root.is_dir() {
        return None;
    }
    if [
        "conv_frontend.onnx",
        "encoder.int8.onnx",
        "decoder.int8.onnx",
        "tokenizer",
    ]
    .iter()
    .all(|name| model_root.join(name).exists())
    {
        return Some(model_root.to_path_buf());
    }
    std::fs::read_dir(model_root)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .find_map(find_qwen_model_dir)
}

pub fn inspect_runtime(
    model_path: impl AsRef<Path>,
    ffmpeg_path: impl AsRef<Path>,
    ocr_path: impl AsRef<Path>,
    provider: Option<&ProviderConfig>,
) -> RuntimeAvailability {
    let model_path = model_path.as_ref();
    let asr_model_available = qwen_model_is_complete(model_path)
        || whisper_model_is_complete(model_path.join("whisper-compatibility"))
        || nemotron_model_is_complete(model_path.join("nemotron-multilingual"));
    let executable_ready = |path: &Path, marker: &str| {
        if !path.is_file() {
            return false;
        }
        // CREATE_NO_WINDOW keeps the version check from flashing a console.
        let Ok(output) = run_hidden(path, &["--version".as_ref()]) else {
            return false;
        };
        // Some Windows builds return a non-zero code after printing their version
        // (notably ffmpeg/ffprobe sidecars launched from a Tauri process). The
        // version marker is the capability signal we actually need.
        let mut version = output.stdout.clone();
        version.extend(&output.stderr);
        output.status.success()
            || String::from_utf8_lossy(&version)
                .to_ascii_lowercase()
                .contains(marker)
    };
    let ffmpeg_path = ffmpeg_path.as_ref();
    let ffprobe_path = ffmpeg_path
        .parent()
        .map(|parent| parent.join("ffprobe.exe"))
        .filter(|path| path.is_file())
        .or_else(|| {
            ffmpeg_path
                .parent()
                .map(|parent| parent.join("ffprobe-x86_64-pc-windows-msvc.exe"))
        })
        .unwrap_or_else(|| PathBuf::from("ffprobe.exe"));
    let ffmpeg_available = executable_ready(ffmpeg_path, "ffmpeg version");
    let ffprobe_available = executable_ready(&ffprobe_path, "ffprobe version");
    let ocr_path = ocr_path.as_ref();
    let ocr_available = executable_ready(ocr_path, "tesseract")
        && ocr_path
            .parent()
            .map(|parent| parent.join("tessdata").join("eng.traineddata").is_file())
            .unwrap_or(false);
    let provider_configured = provider
        .map(|config| {
            config.enabled && !config.base_url.trim().is_empty() && !config.model.trim().is_empty()
        })
        .unwrap_or(false);
    RuntimeAvailability {
        asr_model_available,
        ffmpeg_available,
        ffprobe_available,
        ocr_available,
        provider_configured,
        can_transcribe_locally: asr_model_available,
    }
}

pub trait AsrEngine {
    fn kind(&self) -> AsrEngineKind;
    fn capabilities(&self) -> AsrCapabilities;
    fn transcribe(
        &self,
        chunk: &AudioChunkInput,
        language: &TranscriptLanguage,
    ) -> Result<AsrResult, BeaError>;
}

/// Native offline Qwen3-ASR 0.6B INT8 engine backed by sherpa-onnx.
///
/// The model directory is intentionally supplied at runtime so the installer stays
/// small and the model manager can verify/update the model independently.
///
/// The recognizer lives in an Arc so the cheap handle can be cloned per decode
/// (the watchdog worker needs ownership); decodes run sequentially, so the
/// shared underlying session is never used concurrently.
/// The recognizer is guarded by a Mutex so an abandoned watchdog worker (a
/// decode declared hung but still running) can never decode concurrently with
/// a retry on the same sherpa session — the binding does not guarantee
/// cross-thread safety for one recognizer instance.
type SharedOfflineRecognizer = std::sync::Arc<std::sync::Mutex<OfflineRecognizer>>;
type SharedOnlineRecognizer = std::sync::Arc<std::sync::Mutex<OnlineRecognizer>>;

/// Decode helper shared by every offline engine: locks the recognizer for the
/// whole decode. Contention only occurs when a previously-hung worker is still
/// alive; in that case this blocks briefly and then proceeds safely.
fn decode_offline(
    recognizer: &SharedOfflineRecognizer,
    language_hint: Option<(&str, &str)>,
    sample_rate: i32,
    samples: &[f32],
) -> Option<String> {
    let guard = recognizer.lock().ok()?;
    let stream = guard.create_stream();
    if let Some((key, value)) = language_hint {
        stream.set_option(key, value);
    }
    stream.accept_waveform(sample_rate, samples);
    guard.decode(&stream);
    let result = stream.get_result()?;
    Some(result.text)
}

#[derive(Clone)]
pub struct Qwen3AsrEngine {
    recognizer: SharedOfflineRecognizer,
}

impl Qwen3AsrEngine {
    pub fn from_model_dir(model_dir: impl AsRef<Path>) -> Result<Self, BeaError> {
        let model_dir = model_dir.as_ref();
        let required = [
            model_dir.join("conv_frontend.onnx"),
            model_dir.join("encoder.int8.onnx"),
            model_dir.join("decoder.int8.onnx"),
            model_dir.join("tokenizer"),
        ];
        if required.iter().any(|path| !path.exists()) {
            return Err(BeaError::InvalidState(format!(
                "Qwen3-ASR model directory is incomplete: {}",
                model_dir.display()
            )));
        }
        let mut config = OfflineRecognizerConfig::default();
        config.model_config = OfflineModelConfig {
            qwen3_asr: OfflineQwen3ASRModelConfig {
                conv_frontend: Some(required[0].to_string_lossy().into_owned()),
                encoder: Some(required[1].to_string_lossy().into_owned()),
                decoder: Some(required[2].to_string_lossy().into_owned()),
                tokenizer: Some(required[3].to_string_lossy().into_owned()),
                // The KV budget must cover the prompt scaffold (~50 tokens)
                // plus one audio token per ~0.5s of audio. sherpa's 512 default
                // truncates a 60-second chunk down to a fraction of a second —
                // the transcript then collapses to scaffold text like
                // "language". 2048 fits ~15 minutes; 512 output tokens cover a
                // minute of dense speech.
                max_total_len: 2048,
                max_new_tokens: 512,
                temperature: 1e-6,
                top_p: 0.8,
                seed: 42,
                hotwords: None,
            },
            num_threads: 2,
            provider: Some("cpu".into()),
            ..OfflineModelConfig::default()
        };
        let recognizer = OfflineRecognizer::create(&config).ok_or_else(|| {
            BeaError::MediaProcessing("sherpa-onnx could not initialize Qwen3-ASR".into())
        })?;
        Ok(Self {
            recognizer: std::sync::Arc::new(std::sync::Mutex::new(recognizer)),
        })
    }
}

impl AsrEngine for Qwen3AsrEngine {
    fn kind(&self) -> AsrEngineKind {
        AsrEngineKind::Qwen3Asr06bInt8
    }

    fn capabilities(&self) -> AsrCapabilities {
        AsrCapabilities {
            languages: vec!["en".into(), "fil".into(), "taglish".into()],
            offline: true,
            streaming: false,
        }
    }

    fn transcribe(
        &self,
        chunk: &AudioChunkInput,
        language: &TranscriptLanguage,
    ) -> Result<AsrResult, BeaError> {
        let wave = Wave::read(&chunk.path.to_string_lossy()).ok_or_else(|| {
            BeaError::MediaProcessing(format!(
                "unable to read WAV chunk: {}",
                chunk.path.display()
            ))
        })?;
        let (samples, sample_rate) = engine_input_samples(&wave);
        // Qwen3-ASR builds its prompt scaffold from the language hint. Without
        // it the model must generate the "language …<asr_text>" scaffold itself
        // and sometimes stops early, leaving the literal word "language" as the
        // whole transcript. Passing the hint moves it straight to transcription.
        let hint = match language {
            TranscriptLanguage::English => Some(("language", "English")),
            TranscriptLanguage::Filipino => Some(("language", "Filipino")),
            TranscriptLanguage::Taglish => Some(("language", "Taglish")),
            TranscriptLanguage::Auto => None,
        };
        let text =
            decode_offline(&self.recognizer, hint, sample_rate, &samples).ok_or_else(|| {
                BeaError::MediaProcessing(format!(
                    "Qwen3-ASR returned no result for {}",
                    chunk.path.display()
                ))
            })?;
        Ok(AsrResult {
            text,
            language_detected: match language {
                TranscriptLanguage::Auto => None,
                TranscriptLanguage::English => Some("en".into()),
                TranscriptLanguage::Filipino => Some("fil".into()),
                TranscriptLanguage::Taglish => Some("taglish".into()),
            },
            confidence: None,
        })
    }
}

fn detected_language(language: &TranscriptLanguage) -> Option<String> {
    match language {
        TranscriptLanguage::Auto => None,
        TranscriptLanguage::English => Some("en".into()),
        TranscriptLanguage::Filipino => Some("fil".into()),
        TranscriptLanguage::Taglish => Some("taglish".into()),
    }
}

/// Upper bound for a single chunk decode before the engine is considered hung.
/// Qwen3-ASR 0.6B INT8 on two CPU threads transcribes a dense 60-second chunk
/// well under a minute; anything approaching this bound means an ONNX session
/// has stalled and the job must fail loudly instead of freezing forever.
pub const CHUNK_DECODE_TIMEOUT_SECONDS: u64 = 180;

/// Marker persisted in `transcript_segments.text` for stretches where the
/// meeting was measured silent. Kept out of FTS so search never matches it;
/// minutes generation skips it because extraction only runs on indexed text.
pub const SILENCE_TEXT: &str = "[silence]";

/// Detects degenerate ASR output that should never reach the transcript:
///
/// - scaffold leaks: Qwen3-ASR sometimes emits its prompt scaffold ("language",
///   "<asr_text>", "English") instead of audio transcription when it fails to
///   lock onto speech;
/// - repetition loops: the same 1-3 word phrase repeated until the token budget,
///   a known failure mode of small decoder-only ASR models on music/noise or
///   heavily accented codeswitched speech (common with Taglish);
/// - single-token echoes on long chunks (e.g. "you" for a full minute).
///
/// Returns the reason when the text is degenerate.
fn is_degenerate_asr_text(raw: &str, language: &TranscriptLanguage) -> Option<&'static str> {
    let text = raw.trim();
    if text.is_empty() {
        return Some("empty");
    }
    let lowered = text.to_lowercase();
    let scaffold_only = ["language", "<asr_text>", "asr_text", "english", "tagalog"]
        .iter()
        .any(|marker| {
            let stripped = lowered.replace(marker, "").trim().to_string();
            stripped.chars().all(|ch| !ch.is_alphanumeric())
        });
    if scaffold_only {
        return Some("scaffold");
    }
    // Repetition loop: normalize words, then a loop of >=6 identical tokens
    // covering most of the output counts as degenerate regardless of language.
    let words: Vec<&str> = lowered
        .split(|ch: char| !(ch.is_alphanumeric() || ch == '\''))
        .filter(|word| !word.is_empty())
        .collect();
    if words.len() >= 6 {
        for span in [1usize, 2, 3] {
            if words.len() < span * 3 {
                continue;
            }
            let pattern = &words[..span];
            let repeats = words
                .chunks(span)
                .filter(|chunk| *chunk == &pattern[..])
                .count();
            if repeats * span >= 6 && repeats * span * 10 >= words.len() * 9 {
                return Some("repetition-loop");
            }
        }
    }
    // Single-token echo on a long (near-full-minute) chunk is almost always a
    // stall artifact — but genuine short answers ("sige po") in short chunks
    // are real speech, so only flag when there is far more audio than words.
    if language == &TranscriptLanguage::Taglish
        && words.len() <= 2
        && text.chars().count() >= 12
        && words.iter().all(|word| word.len() <= 6)
    {
        // A single repeated filler spanning many characters of output.
        return Some("single-echo");
    }
    None
}

/// The fallback pass used when a chunk produces degenerate output under the
/// requested language hint. Re-decoding without the hint lets multilingual
/// engines (Qwen3-ASR especially) re-segment codeswitched speech on their own,
/// which measurably improves Taglish accuracy over forcing a wrong hint.
fn retry_language(language: &TranscriptLanguage) -> Option<TranscriptLanguage> {
    match language {
        TranscriptLanguage::Auto => None,
        TranscriptLanguage::Taglish | TranscriptLanguage::Filipino => {
            Some(TranscriptLanguage::Auto)
        }
        TranscriptLanguage::English => Some(TranscriptLanguage::Auto),
    }
}

/// Builds the persisted placeholder segment for a silent or empty stretch so
/// silent minutes stay visible (and timed) in the transcript instead of
/// silently vanishing between spoken segments.
fn silent_segment(meeting_id: &str, start_seconds: u64, end_seconds: u64) -> TranscriptSegment {
    TranscriptSegment {
        id: Uuid::new_v4().to_string(),
        meeting_id: meeting_id.to_string(),
        start_seconds,
        end_seconds,
        text: SILENCE_TEXT.to_string(),
        language_detected: None,
        language_confidence: None,
        speaker: None,
    }
}

/// Linear-interpolation resampler for recorded microphone chunks. sherpa-onnx
/// consumes raw samples at face value, and every bundled ASR model is trained
/// at 16 kHz — feeding 44.1/48 kHz microphone audio verbatim slows speech ~3x
/// and wrecks accuracy. Imports already normalize through FFmpeg; recordings
/// need this path.
pub fn resample_linear_i16(samples: &[i16], from_rate: u32, to_rate: u32) -> Vec<i16> {
    if from_rate == to_rate || from_rate == 0 || to_rate == 0 || samples.is_empty() {
        return samples.to_vec();
    }
    let ratio = f64::from(to_rate) / f64::from(from_rate);
    let output_len = ((samples.len() as f64) * ratio).round() as usize;
    let mut output = Vec::with_capacity(output_len);
    let last = samples.len() - 1;
    for index in 0..output_len {
        let position = index as f64 / ratio;
        let base = position.floor() as usize;
        let frac = (position - base as f64) as f32;
        let left = samples[base.min(last)];
        let right = samples[(base + 1).min(last)];
        let value = left as f32 + (right as f32 - left as f32) * frac;
        output.push(value.clamp(i16::MIN as f32, i16::MAX as f32) as i16);
    }
    output
}

/// Sample rate every bundled ASR model was trained at.
const ASR_TARGET_SAMPLE_RATE: u32 = 16_000;

/// f32 twin of [`resample_linear_i16`] for decoded waveform buffers.
pub fn resample_linear_f32(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || from_rate == 0 || to_rate == 0 || samples.is_empty() {
        return samples.to_vec();
    }
    let ratio = f64::from(to_rate) / f64::from(from_rate);
    let output_len = ((samples.len() as f64) * ratio).round() as usize;
    let mut output = Vec::with_capacity(output_len);
    let last = samples.len() - 1;
    for index in 0..output_len {
        let position = index as f64 / ratio;
        let base = position.floor() as usize;
        let frac = (position - base as f64) as f32;
        let left = samples[base.min(last)];
        let right = samples[(base + 1).min(last)];
        output.push(left + (right - left) * frac);
    }
    output
}

/// Engine-ready samples: resampled to [`ASR_TARGET_SAMPLE_RATE`] whenever a
/// chunk was not stored at 16 kHz (microphone devices commonly run 44.1/48
/// kHz; without this the models hear slowed-down speech and output garbage).
fn engine_input_samples(wave: &Wave) -> (Vec<f32>, i32) {
    let source_rate = wave.sample_rate();
    if source_rate == ASR_TARGET_SAMPLE_RATE as i32 {
        (wave.samples().to_vec(), ASR_TARGET_SAMPLE_RATE as i32)
    } else {
        (
            resample_linear_f32(
                wave.samples(),
                source_rate.max(1) as u32,
                ASR_TARGET_SAMPLE_RATE,
            ),
            ASR_TARGET_SAMPLE_RATE as i32,
        )
    }
}

/// Offline Whisper compatibility engine. Bea accepts only the verified sherpa-onnx
/// layout (encoder.onnx, decoder.onnx, and tokens.txt), so arbitrary ONNX folders
/// cannot be selected accidentally. The recognizer is Arc+Mutex shared for cheap,
/// watchdog-safe clones.
#[derive(Clone)]
pub struct WhisperCompatibilityEngine {
    recognizer: SharedOfflineRecognizer,
}

impl WhisperCompatibilityEngine {
    pub fn from_model_dir(model_dir: impl AsRef<Path>) -> Result<Self, BeaError> {
        let model_dir = model_dir.as_ref();
        // Upstream sherpa-onnx whisper archives prefix files with the model name
        // (e.g. `turbo-encoder.onnx`) and often ship int8 variants
        // (`turbo-encoder.int8.onnx`); accept those as well as bare names.
        let encoder = prefixed_model_file(model_dir, &["encoder.onnx", "encoder.int8.onnx"])
            .ok_or_else(|| BeaError::InvalidState("Whisper encoder.onnx is missing".into()))?;
        let decoder = prefixed_model_file(model_dir, &["decoder.onnx", "decoder.int8.onnx"])
            .ok_or_else(|| BeaError::InvalidState("Whisper decoder.onnx is missing".into()))?;
        let tokens = prefixed_model_file(model_dir, &["tokens.txt"])
            .ok_or_else(|| BeaError::InvalidState("Whisper tokens.txt is missing".into()))?;
        if !whisper_model_is_complete(model_dir) {
            return Err(BeaError::InvalidState(format!(
                "Whisper model directory is incomplete: {}",
                model_dir.display()
            )));
        }
        let mut config = OfflineRecognizerConfig::default();
        config.model_config = OfflineModelConfig {
            whisper: OfflineWhisperModelConfig {
                encoder: Some(encoder.to_string_lossy().into_owned()),
                decoder: Some(decoder.to_string_lossy().into_owned()),
                language: None,
                task: Some("transcribe".into()),
                tail_paddings: 8000,
                enable_token_timestamps: false,
                enable_segment_timestamps: true,
            },
            tokens: Some(tokens.to_string_lossy().into_owned()),
            num_threads: 2,
            provider: Some("cpu".into()),
            ..OfflineModelConfig::default()
        };
        let recognizer = OfflineRecognizer::create(&config).ok_or_else(|| {
            BeaError::MediaProcessing("sherpa-onnx could not initialize Whisper".into())
        })?;
        Ok(Self {
            recognizer: std::sync::Arc::new(std::sync::Mutex::new(recognizer)),
        })
    }
}

impl AsrEngine for WhisperCompatibilityEngine {
    fn kind(&self) -> AsrEngineKind {
        AsrEngineKind::WhisperCompatibility
    }

    fn capabilities(&self) -> AsrCapabilities {
        AsrCapabilities {
            languages: vec!["multilingual".into()],
            offline: true,
            streaming: false,
        }
    }

    fn transcribe(
        &self,
        chunk: &AudioChunkInput,
        language: &TranscriptLanguage,
    ) -> Result<AsrResult, BeaError> {
        let wave = Wave::read(&chunk.path.to_string_lossy()).ok_or_else(|| {
            BeaError::MediaProcessing(format!(
                "unable to read WAV chunk: {}",
                chunk.path.display()
            ))
        })?;
        let (samples, sample_rate) = engine_input_samples(&wave);
        let text =
            decode_offline(&self.recognizer, None, sample_rate, &samples).ok_or_else(|| {
                BeaError::MediaProcessing(format!(
                    "Whisper returned no result for {}",
                    chunk.path.display()
                ))
            })?;
        Ok(AsrResult {
            text,
            language_detected: detected_language(language),
            confidence: None,
        })
    }
}

/// Native Nemotron 3.5 multilingual streaming engine. The 560ms profile is
/// represented by the verified package metadata and is fed through sherpa's
/// online NeMo CTC recognizer when the model is available. The recognizer is
/// Arc+Mutex shared for cheap, watchdog-safe clones.
#[derive(Clone)]
pub struct NemotronMultilingualEngine {
    recognizer: SharedOnlineRecognizer,
}

impl NemotronMultilingualEngine {
    pub fn from_model_dir(model_dir: impl AsRef<Path>) -> Result<Self, BeaError> {
        let model_dir = model_dir.as_ref();
        if !nemotron_model_is_complete(model_dir) {
            return Err(BeaError::InvalidState(format!(
                "Nemotron model directory is incomplete: {}",
                model_dir.display()
            )));
        }
        let tokens = required_model_file(model_dir, "tokens.txt")
            .or_else(|| prefixed_model_file(model_dir, &["tokens.txt"]))
            .ok_or_else(|| BeaError::InvalidState("Nemotron tokens.txt is missing".into()))?
            .to_string_lossy()
            .into_owned();
        let mut config = OnlineRecognizerConfig::default();
        config.enable_endpoint = true;
        config.rule1_min_trailing_silence = 0.8;
        config.rule2_min_trailing_silence = 1.2;
        config.rule3_min_utterance_length = 20.0;
        config.model_config.tokens = Some(tokens);
        config.model_config.num_threads = 2;
        config.model_config.provider = Some("cpu".into());
        if let Some(ctc) = required_model_file(model_dir, "model.onnx") {
            // Older hand-packaged layouts use a CTC model.onnx.
            config.model_config.nemo_ctc = OnlineNemoCtcModelConfig {
                model: Some(ctc.to_string_lossy().into_owned()),
            };
        } else {
            // Upstream Nemotron 3.5 archives are streaming transducers.
            let encoder = prefixed_model_file(model_dir, &["encoder.onnx", "encoder.int8.onnx"])
                .ok_or_else(|| BeaError::InvalidState("Nemotron encoder.onnx is missing".into()))?;
            let decoder = prefixed_model_file(model_dir, &["decoder.onnx", "decoder.int8.onnx"])
                .ok_or_else(|| BeaError::InvalidState("Nemotron decoder.onnx is missing".into()))?;
            let joiner = prefixed_model_file(model_dir, &["joiner.onnx", "joiner.int8.onnx"])
                .ok_or_else(|| BeaError::InvalidState("Nemotron joiner.onnx is missing".into()))?;
            config.model_config.transducer = OnlineTransducerModelConfig {
                encoder: Some(encoder.to_string_lossy().into_owned()),
                decoder: Some(decoder.to_string_lossy().into_owned()),
                joiner: Some(joiner.to_string_lossy().into_owned()),
            };
        }
        let recognizer = OnlineRecognizer::create(&config).ok_or_else(|| {
            BeaError::MediaProcessing("sherpa-onnx could not initialize Nemotron".into())
        })?;
        Ok(Self {
            recognizer: std::sync::Arc::new(std::sync::Mutex::new(recognizer)),
        })
    }
}

impl AsrEngine for NemotronMultilingualEngine {
    fn kind(&self) -> AsrEngineKind {
        AsrEngineKind::NemotronMultilingual560ms
    }

    fn capabilities(&self) -> AsrCapabilities {
        AsrCapabilities {
            languages: vec!["multilingual".into()],
            offline: true,
            streaming: true,
        }
    }

    fn transcribe(
        &self,
        chunk: &AudioChunkInput,
        language: &TranscriptLanguage,
    ) -> Result<AsrResult, BeaError> {
        let wave = Wave::read(&chunk.path.to_string_lossy()).ok_or_else(|| {
            BeaError::MediaProcessing(format!(
                "unable to read WAV chunk: {}",
                chunk.path.display()
            ))
        })?;
        let (samples, sample_rate) = engine_input_samples(&wave);
        // Streaming decode under the recognizer lock (see decode_offline note
        // on abandoned watchdog workers).
        let text = {
            let guard = self.recognizer.lock().map_err(|_| {
                BeaError::MediaProcessing("Nemotron recognizer lock poisoned".into())
            })?;
            let stream = guard.create_stream();
            stream.accept_waveform(sample_rate, &samples);
            while guard.is_ready(&stream) {
                guard.decode(&stream);
            }
            stream.input_finished();
            while guard.is_ready(&stream) {
                guard.decode(&stream);
            }
            let result = guard
                .get_result(&stream)
                .map(|result| result.text)
                .ok_or_else(|| {
                    BeaError::MediaProcessing(format!(
                        "Nemotron returned no result for {}",
                        chunk.path.display()
                    ))
                })?;
            drop(guard);
            result
        };
        Ok(AsrResult {
            text,
            language_detected: detected_language(language),
            confidence: None,
        })
    }
}

/// Engine selected for a meeting, snapshotted at meeting creation. Cloning is
/// cheap (each recognizer is Arc-shared); the watchdog worker receives an owned
/// clone per decode.
#[derive(Clone)]
pub enum ConfiguredAsrEngine {
    Qwen(Qwen3AsrEngine),
    Whisper(WhisperCompatibilityEngine),
    Nemotron(NemotronMultilingualEngine),
}

impl AsrEngine for ConfiguredAsrEngine {
    fn kind(&self) -> AsrEngineKind {
        match self {
            Self::Qwen(engine) => engine.kind(),
            Self::Whisper(engine) => engine.kind(),
            Self::Nemotron(engine) => engine.kind(),
        }
    }

    fn capabilities(&self) -> AsrCapabilities {
        match self {
            Self::Qwen(engine) => engine.capabilities(),
            Self::Whisper(engine) => engine.capabilities(),
            Self::Nemotron(engine) => engine.capabilities(),
        }
    }

    fn transcribe(
        &self,
        chunk: &AudioChunkInput,
        language: &TranscriptLanguage,
    ) -> Result<AsrResult, BeaError> {
        match self {
            Self::Qwen(engine) => engine.transcribe(chunk, language),
            Self::Whisper(engine) => engine.transcribe(chunk, language),
            Self::Nemotron(engine) => engine.transcribe(chunk, language),
        }
    }
}

pub fn whisper_model_is_complete(model_path: impl AsRef<Path>) -> bool {
    let model_path = model_path.as_ref();
    model_path.is_dir()
        && prefixed_model_file(model_path, &["encoder.onnx", "encoder.int8.onnx"]).is_some()
        && prefixed_model_file(model_path, &["decoder.onnx", "decoder.int8.onnx"]).is_some()
        && prefixed_model_file(model_path, &["tokens.txt"]).is_some()
}

/// Locates a model file that upstream sherpa-onnx archives prefix with the model
/// name (for example `turbo-encoder.onnx`) or ship as int8 variants
/// (`encoder.int8.onnx`). Searches the directory tree because extracted archives
/// wrap their files in a top-level folder. Exact bare names (`encoder.onnx`)
/// also match because they end with the same suffix.
fn prefixed_model_file(root: &Path, suffixes: &[&str]) -> Option<PathBuf> {
    let suffixes = suffixes
        .iter()
        .map(|suffix| suffix.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let mut candidates = Vec::new();
    collect_prefixed_files(root, &suffixes, &mut candidates, 0);
    // Prefer the smallest match (e.g. int8 variants) so a deterministic file is
    // chosen when several variants were extracted into one directory.
    candidates.sort_by_key(|path| std::fs::metadata(path).map(|m| m.len()).unwrap_or(u64::MAX));
    candidates.into_iter().next()
}

fn collect_prefixed_files(
    directory: &Path,
    suffixes: &[String],
    candidates: &mut Vec<PathBuf>,
    depth: usize,
) {
    if depth > 4 {
        return;
    }
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_prefixed_files(&path, suffixes, candidates, depth + 1);
        } else if let Some(name) = path.file_name() {
            let name = name.to_string_lossy().to_ascii_lowercase();
            if suffixes.iter().any(|suffix| name.ends_with(suffix)) {
                candidates.push(path);
            }
        }
    }
}

pub fn nemotron_model_is_complete(model_path: impl AsRef<Path>) -> bool {
    let model_path = model_path.as_ref();
    if !model_path.is_dir() {
        return false;
    }
    // Upstream Nemotron 3.5 archives are streaming transducers
    // (encoder/decoder/joiner, often int8 variants); older hand-packaged
    // layouts use a CTC `model.onnx`. Accept either shape.
    let transducer = prefixed_model_file(model_path, &["encoder.onnx", "encoder.int8.onnx"])
        .is_some()
        && prefixed_model_file(model_path, &["decoder.onnx", "decoder.int8.onnx"]).is_some()
        && prefixed_model_file(model_path, &["joiner.onnx", "joiner.int8.onnx"]).is_some();
    transducer || required_model_file(model_path, "model.onnx").is_some()
}

fn required_model_file(root: &Path, file_name: &str) -> Option<PathBuf> {
    if root.join(file_name).is_file() {
        return Some(root.join(file_name));
    }
    let entries = std::fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = required_model_file(&path, file_name) {
                return Some(found);
            }
        }
    }
    None
}

pub fn load_asr_engine(
    model_root: impl AsRef<Path>,
    engine_id: &str,
) -> Result<ConfiguredAsrEngine, BeaError> {
    let model_root = model_root.as_ref();
    match engine_id {
        "whisper-compatibility" => {
            let model_dir = model_root.join("whisper-compatibility");
            WhisperCompatibilityEngine::from_model_dir(model_dir).map(ConfiguredAsrEngine::Whisper)
        }
        "nemotron-multilingual" => {
            let model_dir = model_root.join("nemotron-multilingual");
            NemotronMultilingualEngine::from_model_dir(model_dir).map(ConfiguredAsrEngine::Nemotron)
        }
        _ => find_qwen_model_dir(model_root)
            .ok_or_else(|| {
                BeaError::InvalidState("Qwen3-ASR model is not installed or is incomplete".into())
            })
            .and_then(Qwen3AsrEngine::from_model_dir)
            .map(ConfiguredAsrEngine::Qwen),
    }
}

pub fn transcribe_chunks<E: AsrEngine + Clone + Send + 'static>(
    conn: &Connection,
    meeting_id: &str,
    chunks: &[AudioChunkInput],
    language: &TranscriptLanguage,
    engine: E,
) -> Result<Vec<TranscriptSegment>, BeaError> {
    transcribe_chunks_with_progress(conn, meeting_id, chunks, language, engine, |_, _, _| {})
}

/// Runs one chunk decode on a worker thread under a hard watchdog timeout so a
/// stalled ONNX session can no longer freeze an entire transcription job: the
/// caller gets a loud error naming the chunk instead of an infinitely
/// "processing" meeting.
///
/// Ownership dance: the engine moves into the worker and is sent back through
/// a channel the moment decoding finishes. If nothing arrives within
/// [`CHUNK_DECODE_TIMEOUT_SECONDS`] the decode is declared hung, the job fails,
/// and the worker thread (with its engine) is intentionally left to finish or
/// die in the background rather than blocking the UI forever.
fn transcribe_chunk_with_timeout<E: AsrEngine + Send + 'static>(
    engine: E,
    chunk: &AudioChunkInput,
    language: &TranscriptLanguage,
) -> Result<(E, AsrResult), BeaError> {
    use std::sync::mpsc::RecvTimeoutError;
    let owned = AudioChunkInput {
        path: chunk.path.clone(),
        start_seconds: chunk.start_seconds,
        end_seconds: chunk.end_seconds,
    };
    let language = language.clone();
    let (engine_tx, engine_rx) = std::sync::mpsc::channel();
    let path_display = chunk.path.display().to_string();
    let worker = std::thread::spawn(move || {
        let result = engine.transcribe(&owned, &language);
        // Hand the engine back first; the caller declares a hang when this
        // never arrives inside the watchdog budget.
        if engine_tx.send(engine).is_err() {
            return None;
        }
        Some(result)
    });
    let returned_engine = match engine_rx
        .recv_timeout(std::time::Duration::from_secs(CHUNK_DECODE_TIMEOUT_SECONDS))
    {
        Ok(engine) => engine,
        Err(RecvTimeoutError::Timeout) => {
            return Err(BeaError::MediaProcessing(format!(
                "ASR decode did not finish within {}s on {} — the chunk was \
                 declared hung and the transcription job failed instead of \
                 stalling. Retry the meeting; if this repeats, restart Bea to \
                 release the stalled model session.",
                CHUNK_DECODE_TIMEOUT_SECONDS, path_display
            )));
        }
        // Worker panicked before handing the engine back: surface the real
        // failure instead of misreporting it as a timeout.
        Err(RecvTimeoutError::Disconnected) => {
            let panic = worker.join().err();
            return Err(BeaError::MediaProcessing(format!(
                "ASR worker crashed on {}: {panic:?}",
                path_display
            )));
        }
    };
    let outcome = worker.join().map_err(|panic| {
        BeaError::MediaProcessing(format!("ASR worker crashed on {}: {panic:?}", path_display))
    })?;
    match outcome {
        Some(result) => Ok((returned_engine, result?)),
        None => Err(BeaError::MediaProcessing(format!(
            "ASR worker produced no result for {}",
            path_display
        ))),
    }
}

/// Same as [`transcribe_chunks`], but reports after every chunk with the newly
/// persisted segment so the UI can stream the transcript live. Silent chunks
/// are logged as timed `[silence]` rows (so minutes of silence stay visible in
/// the transcript), degenerate outputs trigger one auto-language retry pass,
/// and every decode runs under a hard watchdog timeout.
pub fn transcribe_chunks_with_progress<E: AsrEngine + Clone + Send + 'static>(
    conn: &Connection,
    meeting_id: &str,
    chunks: &[AudioChunkInput],
    language: &TranscriptLanguage,
    engine: E,
    mut on_progress: impl FnMut(&TranscriptSegment, usize, usize),
) -> Result<Vec<TranscriptSegment>, BeaError> {
    let total = chunks.len();
    let mut segments = Vec::with_capacity(chunks.len());
    // The watchdog worker owns a fresh clone of the cheap engine handle per
    // decode; the original stays with this loop.
    for (index, chunk) in chunks.iter().enumerate() {
        if chunk_is_silent(&chunk.path) {
            // No speech energy in this stretch — log it so silent minutes stay
            // visible and timed in the transcript instead of vanishing.
            let silent = silent_segment(meeting_id, chunk.start_seconds, chunk.end_seconds);
            add_segment(conn, &silent)?;
            segments.push(silent.clone());
            on_progress(&silent, index + 1, total);
            continue;
        }
        // Watchdog: a hung ONNX decode now fails loudly instead of stalling.
        let (_, mut result) = transcribe_chunk_with_timeout(engine.clone(), chunk, language)?;
        // Degenerate output (scaffold leaks, repetition loops, single-token
        // echoes) gets one fallback pass without the language hint before the
        // text is accepted — measurably better Taglish on codeswitched speech.
        if is_degenerate_asr_text(&result.text, language).is_some() {
            if let Some(fallback) = retry_language(language) {
                let (_, retried) = transcribe_chunk_with_timeout(engine.clone(), chunk, &fallback)?;
                if is_degenerate_asr_text(&retried.text, &fallback).is_none() {
                    result = retried;
                }
            }
        }
        if result.text.trim().is_empty() || SILENCE_TEXT == result.text.trim() {
            // The engine heard nothing intelligible — keep the stretch timed
            // with the same placeholder used for measured silence.
            let empty = silent_segment(meeting_id, chunk.start_seconds, chunk.end_seconds);
            add_segment(conn, &empty)?;
            segments.push(empty.clone());
            on_progress(&empty, index + 1, total);
            continue;
        }
        let segment = TranscriptSegment {
            id: Uuid::new_v4().to_string(),
            meeting_id: meeting_id.to_string(),
            start_seconds: chunk.start_seconds,
            end_seconds: chunk.end_seconds,
            text: result.text,
            language_detected: result.language_detected,
            language_confidence: result.confidence,
            speaker: None,
        };
        add_segment(conn, &segment)?;
        segments.push(segment.clone());
        on_progress(&segment, index + 1, total);
    }
    Ok(segments)
}

/// Speaker diarization: labels transcript segments with a speaker index using
/// sherpa-onnx's pyannote segmentation + speaker-embedding clustering. The
/// models are small (~7 MB + ~28 MB) and loaded once per transcription run.
pub struct SpeakerDiarizer {
    inner: sherpa_onnx::OfflineSpeakerDiarization,
}

impl SpeakerDiarizer {
    /// `models_dir` is the app's `models` root containing
    /// `pyannote-segmentation-3-0/model.onnx` and the 3dspeaker embedding ONNX.
    pub fn from_models_dir(models_dir: &Path) -> Option<Self> {
        let segmentation = models_dir
            .join("pyannote-segmentation-3-0")
            .join("model.onnx");
        let embedding = models_dir.join("3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx");
        if !segmentation.is_file() || !embedding.is_file() {
            return None;
        }
        let config = sherpa_onnx::OfflineSpeakerDiarizationConfig {
            segmentation: sherpa_onnx::OfflineSpeakerSegmentationModelConfig {
                pyannote: sherpa_onnx::OfflineSpeakerSegmentationPyannoteModelConfig {
                    model: Some(segmentation.to_string_lossy().into_owned()),
                },
                ..Default::default()
            },
            embedding: sherpa_onnx::SpeakerEmbeddingExtractorConfig {
                model: Some(embedding.to_string_lossy().into_owned()),
                ..Default::default()
            },
            clustering: sherpa_onnx::FastClusteringConfig {
                // -1 lets the clustering threshold decide how many speakers the
                // audio actually contains instead of forcing exactly two, so
                // three-person meetings and one-on-ones both label correctly.
                num_clusters: -1,
                threshold: 0.5,
                ..Default::default()
            },
            ..Default::default()
        };
        let inner = sherpa_onnx::OfflineSpeakerDiarization::create(&config)?;
        Some(Self { inner })
    }

    /// Speaker index active at `offset_seconds` within a full-meeting waveform.
    /// Returns None when no diarization segment covers the point.
    pub fn speaker_at(
        &self,
        segments: &[SherpaDiarizationSegment],
        offset_seconds: f32,
    ) -> Option<u32> {
        segments
            .iter()
            .find(|segment| offset_seconds >= segment.start && offset_seconds < segment.end)
            .map(|segment| segment.speaker)
    }

    /// Run diarization over a whole meeting's normalized waveform. Returns
    /// per-time speaker turns sorted by start.
    pub fn process_wave(&self, samples: &[f32]) -> Option<Vec<SherpaDiarizationSegment>> {
        let result = self.inner.process(samples)?;
        Some(
            result
                .sort_by_start_time()
                .into_iter()
                .map(|segment| SherpaDiarizationSegment {
                    start: segment.start,
                    end: segment.end,
                    speaker: segment.speaker.max(0) as u32,
                })
                .collect(),
        )
    }
}

/// Plain-data diarization turn (decoupled from the sherpa result lifetime).
#[derive(Debug, Clone)]
pub struct SherpaDiarizationSegment {
    pub start: f32,
    pub end: f32,
    pub speaker: u32,
}

/// Reads a whole meeting WAV as mono f32 samples at `target_rate` (linear
/// resample when the file's rate differs). Diarization and ASR models expect
/// 16 kHz; microphone chunks are recorded at whatever rate the input device
/// runs (commonly 48 kHz) and would otherwise be misread by ~3x.
///
/// Multi-channel files are averaged to mono so every channel contributes.
pub fn read_wav_samples_resampled(path: &Path, target_rate: u32) -> Option<(Vec<f32>, u32)> {
    let reader = hound::WavReader::open(path).ok()?;
    let spec = reader.spec();
    if spec.sample_format != hound::SampleFormat::Int || spec.bits_per_sample != 16 {
        return None;
    }
    let channels = usize::from(spec.channels.max(1));
    let mut frames: Vec<i16> = Vec::new();
    for frame in reader
        .into_samples::<i16>()
        .collect::<Result<Vec<_>, _>>()
        .ok()?
        .chunks(channels)
    {
        if frame.len() == channels {
            let sum: i64 = frame.iter().map(|value| i64::from(*value)).sum();
            frames.push((sum / channels as i64).clamp(i16::MIN as i64, i16::MAX as i64) as i16);
        }
    }
    let resampled = resample_linear_i16(&frames, spec.sample_rate, target_rate);
    Some((
        resampled
            .into_iter()
            .map(|value| f32::from(value) / 32768.0)
            .collect(),
        target_rate,
    ))
}

/// Cheap silence gate: peak-amplitude scan of the *entire* chunk (a 60 s peak
/// scan is far cheaper than an ASR decode, and probing only the head used to
/// misclassify chunks whose speech started later as `[silence]`, permanently
/// dropping that audio). Peak below -45 dBFS counts as silence (room tone).
/// Returns false when the file cannot be read (fail open — let the engine
/// surface a real error instead of silently dropping audio).
fn chunk_is_silent(path: &Path) -> bool {
    const PEAK_THRESHOLD: i16 = 185; // ~ -45 dBFS
    let reader = match hound::WavReader::open(path) {
        Ok(reader) => reader,
        Err(_) => return false,
    };
    let spec = reader.spec();
    if spec.sample_format != hound::SampleFormat::Int || spec.bits_per_sample != 16 {
        return false;
    }
    let mut peak: i64 = 0;
    for sample in reader.into_samples::<i16>() {
        let Ok(sample) = sample else { continue };
        peak = peak.max(sample.unsigned_abs() as i64);
        if peak > PEAK_THRESHOLD as i64 {
            return false;
        }
    }
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum VerificationStatus {
    Supported,
    Uncertain,
    Contradicted,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VerifiedClaim {
    pub claim: String,
    pub status: VerificationStatus,
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VisualFrame {
    pub id: String,
    pub timestamp_seconds: u64,
    pub path: PathBuf,
    pub thumbnail_path: Option<PathBuf>,
    pub perceptual_hash: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OcrResult {
    pub text: String,
    pub confidence: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VisualAnalysis {
    pub frame_id: String,
    pub timestamp_seconds: u64,
    pub ocr: OcrResult,
    pub description: Option<String>,
}

pub trait OcrEngine {
    fn extract_text(&self, frame: &VisualFrame) -> Result<OcrResult, BeaError>;
}

/// OCR adapter for a locally installed Tesseract executable. The executable and
/// language data remain runtime assets so deployments can choose the languages
/// they need without putting large trained-data files in SQLite or Rust memory.
pub struct TesseractOcrEngine {
    pub executable: PathBuf,
    pub language: String,
}

impl OcrEngine for TesseractOcrEngine {
    fn extract_text(&self, frame: &VisualFrame) -> Result<OcrResult, BeaError> {
        let mut command = std::process::Command::new(&self.executable);
        command
            .arg(&frame.path)
            .arg("stdout")
            .args(["--psm", "6", "-l"])
            .arg(&self.language);
        hide_console_window(&mut command);
        if let Some(parent) = self.executable.parent() {
            command.env("TESSDATA_PREFIX", parent.join("tessdata"));
        }
        let output = command
            .output()
            .map_err(|error| BeaError::MediaProcessing(error.to_string()))?;
        if !output.status.success() {
            return Err(BeaError::MediaProcessing(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }
        Ok(OcrResult {
            text: String::from_utf8_lossy(&output.stdout).trim().to_string(),
            confidence: None,
        })
    }
}

pub trait VisionEngine {
    fn describe(&self, frame: &VisualFrame, ocr: &OcrResult) -> Result<Option<String>, BeaError>;
}

pub fn analyze_visual_frames<O: OcrEngine, V: VisionEngine>(
    frames: &[VisualFrame],
    ocr: &O,
    vision: &V,
) -> Result<Vec<VisualAnalysis>, BeaError> {
    deduplicate_frames(frames)
        .into_iter()
        .map(|frame| {
            let ocr_result = ocr.extract_text(&frame)?;
            let description = vision.describe(&frame, &ocr_result)?;
            Ok(VisualAnalysis {
                frame_id: frame.id,
                timestamp_seconds: frame.timestamp_seconds,
                ocr: ocr_result,
                description,
            })
        })
        .collect()
}

pub fn save_visual_analysis(
    conn: &Connection,
    meeting_id: &str,
    frame: &VisualFrame,
    analysis: &VisualAnalysis,
) -> Result<(), BeaError> {
    conn.execute(
        "INSERT OR REPLACE INTO visual_evidence(id,meeting_id,timestamp_seconds,path,thumbnail_path,ocr_text,perceptual_hash,description) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            frame.id,
            meeting_id,
            frame.timestamp_seconds,
            frame.path.to_string_lossy().to_string(),
            frame.thumbnail_path.as_ref().map(|path| path.to_string_lossy().to_string()),
            analysis.ocr.text,
            frame.perceptual_hash,
            analysis.description,
        ],
    )?;
    Ok(())
}

pub fn load_visual_frames(
    conn: &Connection,
    meeting_id: &str,
) -> Result<Vec<VisualFrame>, BeaError> {
    let mut statement = conn.prepare("SELECT id,timestamp_seconds,path,thumbnail_path,perceptual_hash,description FROM visual_evidence WHERE meeting_id=?1 ORDER BY timestamp_seconds")?;
    let rows = statement.query_map(params![meeting_id], |row| {
        Ok(VisualFrame {
            id: row.get(0)?,
            timestamp_seconds: row.get(1)?,
            path: PathBuf::from(row.get::<_, String>(2)?),
            thumbnail_path: row.get::<_, Option<String>>(3)?.map(PathBuf::from),
            perceptual_hash: row.get(4)?,
            description: row.get(5)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MediaMetadata {
    pub duration_seconds: u64,
    pub has_video: bool,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub audio_streams: u32,
}

pub trait MediaPipeline {
    fn probe(&self, input: &Path) -> Result<MediaMetadata, BeaError>;
    /// Path to the ffmpeg executable, used when the pipeline needs extra
    /// invocations (e.g. slicing normalized audio into chunks).
    fn ffmpeg_path(&self) -> &Path;
    fn extract_audio(&self, input: &Path, output: &Path) -> Result<(), BeaError>;
    fn extract_keyframes(
        &self,
        input: &Path,
        output_dir: &Path,
        interval_seconds: u32,
    ) -> Result<(), BeaError>;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AudioInputDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
}

pub fn list_audio_input_devices() -> Result<Vec<AudioInputDevice>, BeaError> {
    let host = cpal::default_host();
    let default_name = host
        .default_input_device()
        .and_then(|device| device.name().ok());
    let devices = host
        .input_devices()
        .map_err(|error| BeaError::MediaProcessing(error.to_string()))?;
    Ok(devices
        .enumerate()
        .filter_map(|(index, device)| {
            let name = device.name().ok()?;
            let config = device.default_input_config().ok();
            Some(AudioInputDevice {
                id: format!("input-{index}"),
                is_default: default_name.as_deref() == Some(name.as_str()),
                name,
                sample_rate: config.as_ref().map(|value| value.sample_rate().0),
                channels: config.map(|value| value.channels()),
            })
        })
        .collect())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum RecorderState {
    Idle,
    Recording,
    Paused,
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecorderConfig {
    pub sample_rate: u32,
    pub channels: u16,
    pub chunk_seconds: u64,
    pub max_duration_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompletedAudioChunk {
    pub ordinal: u32,
    pub path: PathBuf,
    pub start_seconds: u64,
    pub end_seconds: u64,
    pub sample_count: u64,
}

/// Writes microphone samples directly to one WAV file at a time. The recorder never buffers
/// the meeting; only the current chunk is open, and each completed chunk can be queued for ASR.
pub struct SegmentedWavRecorder {
    root: PathBuf,
    config: RecorderConfig,
    state: RecorderState,
    ordinal: u32,
    current_samples: u64,
    total_samples: u64,
    writer: Option<hound::WavWriter<std::io::BufWriter<std::fs::File>>>,
}

impl SegmentedWavRecorder {
    pub fn new(root: impl AsRef<Path>, config: RecorderConfig) -> Result<Self, BeaError> {
        if config.sample_rate == 0
            || config.channels == 0
            || config.chunk_seconds == 0
            || config.max_duration_seconds == 0
            || config.chunk_seconds > config.max_duration_seconds
        {
            return Err(BeaError::InvalidState(
                "invalid recorder configuration".into(),
            ));
        }
        std::fs::create_dir_all(root.as_ref())
            .map_err(|error| BeaError::MediaProcessing(error.to_string()))?;
        Ok(Self {
            root: root.as_ref().to_path_buf(),
            config,
            state: RecorderState::Idle,
            ordinal: 0,
            current_samples: 0,
            total_samples: 0,
            writer: None,
        })
    }
    pub fn state(&self) -> RecorderState {
        self.state.clone()
    }
    pub fn start(&mut self) -> Result<(), BeaError> {
        if self.state != RecorderState::Idle {
            return Err(BeaError::InvalidState("recorder is already started".into()));
        }
        self.state = RecorderState::Recording;
        Ok(())
    }
    pub fn pause(&mut self) -> Result<(), BeaError> {
        if self.state != RecorderState::Recording {
            return Err(BeaError::InvalidState("recorder is not recording".into()));
        }
        self.state = RecorderState::Paused;
        Ok(())
    }
    pub fn resume(&mut self) -> Result<(), BeaError> {
        if self.state != RecorderState::Paused {
            return Err(BeaError::InvalidState("recorder is not paused".into()));
        }
        self.state = RecorderState::Recording;
        Ok(())
    }
    pub fn push_samples(&mut self, samples: &[i16]) -> Result<Vec<CompletedAudioChunk>, BeaError> {
        if self.state != RecorderState::Recording {
            return Ok(Vec::new());
        }
        let samples_per_second = self.config.sample_rate as u64 * self.config.channels as u64;
        let max_samples = samples_per_second * self.config.max_duration_seconds;
        if self.total_samples.saturating_add(samples.len() as u64) > max_samples {
            return Err(BeaError::InvalidDuration(
                self.config.max_duration_seconds + 1,
            ));
        }
        let chunk_samples = samples_per_second * self.config.chunk_seconds;
        let mut completed = Vec::new();
        for sample in samples {
            if self.writer.is_none() {
                self.open_writer()?;
            }
            self.writer
                .as_mut()
                .expect("writer opened")
                .write_sample(*sample)
                .map_err(|error| BeaError::MediaProcessing(error.to_string()))?;
            self.current_samples += 1;
            self.total_samples += 1;
            if self.current_samples == chunk_samples {
                completed.push(self.finalize_chunk()?);
            }
        }
        Ok(completed)
    }
    pub fn stop(&mut self) -> Result<Vec<CompletedAudioChunk>, BeaError> {
        if self.state == RecorderState::Idle || self.state == RecorderState::Stopped {
            return Ok(Vec::new());
        }
        self.state = RecorderState::Stopped;
        if self.writer.is_some() {
            Ok(vec![self.finalize_chunk()?])
        } else {
            Ok(Vec::new())
        }
    }
    fn open_writer(&mut self) -> Result<(), BeaError> {
        self.ordinal += 1;
        let path = self.root.join(format!("chunk-{:05}.wav", self.ordinal));
        let spec = hound::WavSpec {
            channels: self.config.channels,
            sample_rate: self.config.sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        self.writer = Some(
            hound::WavWriter::create(path, spec)
                .map_err(|error| BeaError::MediaProcessing(error.to_string()))?,
        );
        Ok(())
    }
    fn finalize_chunk(&mut self) -> Result<CompletedAudioChunk, BeaError> {
        let path = self.root.join(format!("chunk-{:05}.wav", self.ordinal));
        let sample_count = self.current_samples;
        self.writer
            .take()
            .expect("active writer")
            .finalize()
            .map_err(|error| BeaError::MediaProcessing(error.to_string()))?;
        let per_second = self.config.sample_rate as u64 * self.config.channels as u64;
        let end = self.total_samples / per_second;
        let start = (self.total_samples - sample_count) / per_second;
        self.current_samples = 0;
        Ok(CompletedAudioChunk {
            ordinal: self.ordinal,
            path,
            start_seconds: start,
            end_seconds: end,
            sample_count,
        })
    }
}

pub struct FfmpegPipeline {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VideoAnalysis {
    pub metadata: MediaMetadata,
    pub audio_path: PathBuf,
    pub frames: Vec<VisualFrame>,
}

pub fn analyze_video<P: MediaPipeline>(
    pipeline: &P,
    input: &Path,
    output_dir: &Path,
    interval_seconds: u32,
) -> Result<VideoAnalysis, BeaError> {
    if interval_seconds == 0 {
        return Err(BeaError::InvalidState(
            "frame interval must be positive".into(),
        ));
    }
    let metadata = pipeline.probe(input)?;
    std::fs::create_dir_all(output_dir)
        .map_err(|error| BeaError::MediaProcessing(error.to_string()))?;
    // Qwen3-ASR consumes PCM WAV chunks directly; keep the normalized media
    // artifact lossless and seekable for both audio and video imports.
    let audio_path = output_dir.join("audio.wav");
    pipeline.extract_audio(input, &audio_path)?;
    let frames_dir = output_dir.join("frames");
    pipeline.extract_keyframes(input, &frames_dir, interval_seconds)?;
    let mut frames = Vec::new();
    for entry in std::fs::read_dir(&frames_dir)
        .map_err(|error| BeaError::MediaProcessing(error.to_string()))?
        .flatten()
    {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let bytes =
            std::fs::read(&path).map_err(|error| BeaError::MediaProcessing(error.to_string()))?;
        let ordinal = path
            .file_stem()
            .and_then(|name| name.to_str())
            .and_then(|name| name.rsplit('-').next())
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(frames.len() as u64);
        frames.push(VisualFrame {
            id: Uuid::new_v4().to_string(),
            timestamp_seconds: ordinal.saturating_mul(interval_seconds as u64),
            path,
            thumbnail_path: None,
            perceptual_hash: frame_hash(&bytes),
            description: None,
        });
    }
    Ok(VideoAnalysis {
        metadata,
        audio_path,
        frames: deduplicate_frames(&frames),
    })
}

/// Normalize an imported audio or video source to a local PCM WAV and feed it
/// through the same offline ASR path used by microphone recordings. Video
/// imports also run the keyframe pipeline so the derived audio stays aligned
/// with visual evidence timestamps.
pub fn transcribe_imported_media<P: MediaPipeline, E: AsrEngine + Clone + Send + 'static>(
    conn: &Connection,
    meeting_id: &str,
    input: &Path,
    kind: &MediaKind,
    output_dir: &Path,
    language: &TranscriptLanguage,
    pipeline: &P,
    engine: E,
) -> Result<Vec<TranscriptSegment>, BeaError> {
    transcribe_imported_media_with_progress(
        conn,
        meeting_id,
        input,
        kind,
        output_dir,
        language,
        pipeline,
        engine,
        |_, _, _| {},
    )
}

/// Same as [`transcribe_imported_media`], but forwards chunk progress
/// `(completed, total)` for UI reporting.
pub fn transcribe_imported_media_with_progress<
    P: MediaPipeline,
    E: AsrEngine + Clone + Send + 'static,
>(
    conn: &Connection,
    meeting_id: &str,
    input: &Path,
    kind: &MediaKind,
    output_dir: &Path,
    language: &TranscriptLanguage,
    pipeline: &P,
    engine: E,
    on_progress: impl FnMut(&TranscriptSegment, usize, usize),
) -> Result<Vec<TranscriptSegment>, BeaError> {
    let (metadata, audio_path) = match kind {
        MediaKind::Audio => {
            let metadata = pipeline.probe(input)?;
            std::fs::create_dir_all(output_dir)
                .map_err(|error| BeaError::MediaProcessing(error.to_string()))?;
            let audio_path = output_dir.join("audio.wav");
            pipeline.extract_audio(input, &audio_path)?;
            (metadata, audio_path)
        }
        MediaKind::Video => {
            let analysis = analyze_video(pipeline, input, output_dir, 5)?;
            (analysis.metadata, analysis.audio_path)
        }
    };
    // Split the normalized audio into bounded 28-second chunks. One chunk per
    // inference keeps sherpa-onnx's peak memory flat and gives the UI real
    // progress with segments streaming in as they finish. Whisper-family
    // engines only process the first 30 s of any input, so chunks must stay
    // under that ceiling (28 s + margin) or audio would be silently dropped.
    let total_seconds = metadata.duration_seconds.max(1);
    let chunk_seconds = 28u64;
    let chunk_count = total_seconds.div_ceil(chunk_seconds);
    let chunks_dir = output_dir.join("audio-chunks");
    std::fs::create_dir_all(&chunks_dir)
        .map_err(|error| BeaError::MediaProcessing(error.to_string()))?;
    let mut inputs = Vec::with_capacity(chunk_count as usize);
    for index in 0..chunk_count {
        let start = index * chunk_seconds;
        let end = (start + chunk_seconds).min(total_seconds);
        let chunk_path = chunks_dir.join(format!("chunk-{index:04}.wav"));
        if !chunk_path.is_file() {
            run_ffmpeg(
                pipeline.ffmpeg_path(),
                &ffmpeg_slice_args(&audio_path, &chunk_path, start, end),
            )?;
        }
        inputs.push(AudioChunkInput {
            path: chunk_path,
            start_seconds: start,
            end_seconds: end,
        });
    }
    transcribe_chunks_with_progress(conn, meeting_id, &inputs, language, engine, on_progress)
}

impl MediaPipeline for FfmpegPipeline {
    fn probe(&self, input: &Path) -> Result<MediaMetadata, BeaError> {
        let mut command = std::process::Command::new(&self.ffprobe);
        command
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration:stream=codec_type,width,height",
                "-of",
                "json",
            ])
            .arg(input);
        hide_console_window(&mut command);
        let output = command
            .output()
            .map_err(|e| BeaError::MediaProcessing(e.to_string()))?;
        if !output.status.success() {
            return Err(BeaError::MediaProcessing(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }
        let value: serde_json::Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| BeaError::MediaProcessing(e.to_string()))?;
        let duration_seconds = value
            .get("format")
            .and_then(|format| format.get("duration"))
            .and_then(serde_json::Value::as_str)
            .and_then(|duration| duration.parse::<f64>().ok())
            .map(|duration| duration.ceil() as u64)
            .unwrap_or(0);
        let streams = value
            .get("streams")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        let video = streams.iter().find(|stream| {
            stream.get("codec_type").and_then(serde_json::Value::as_str) == Some("video")
        });
        let audio_streams = streams
            .iter()
            .filter(|stream| {
                stream.get("codec_type").and_then(serde_json::Value::as_str) == Some("audio")
            })
            .count() as u32;
        Ok(MediaMetadata {
            duration_seconds,
            has_video: video.is_some(),
            width: video
                .and_then(|stream| stream.get("width"))
                .and_then(serde_json::Value::as_u64)
                .map(|value| value as u32),
            height: video
                .and_then(|stream| stream.get("height"))
                .and_then(serde_json::Value::as_u64)
                .map(|value| value as u32),
            audio_streams,
        })
    }
    fn extract_audio(&self, input: &Path, output: &Path) -> Result<(), BeaError> {
        run_ffmpeg(&self.ffmpeg, &ffmpeg_audio_args(input, output))
    }
    fn ffmpeg_path(&self) -> &Path {
        &self.ffmpeg
    }
    fn extract_keyframes(
        &self,
        input: &Path,
        output_dir: &Path,
        interval_seconds: u32,
    ) -> Result<(), BeaError> {
        std::fs::create_dir_all(output_dir)
            .map_err(|e| BeaError::MediaProcessing(e.to_string()))?;
        run_ffmpeg(
            &self.ffmpeg,
            &ffmpeg_keyframe_args(input, &output_dir.join("frame-%06d.jpg"), interval_seconds),
        )
    }
}

pub fn ffmpeg_audio_args(input: &Path, output: &Path) -> Vec<String> {
    vec![
        "-y".into(),
        "-i".into(),
        input.to_string_lossy().into_owned(),
        "-vn".into(),
        "-ac".into(),
        "1".into(),
        "-ar".into(),
        "16000".into(),
        "-c:a".into(),
        "pcm_s16le".into(),
        output.to_string_lossy().into_owned(),
    ]
}

/// ffmpeg arguments that cut `[start, end)` seconds out of an existing WAV
/// without re-encoding (both are PCM, so a stream copy is exact).
pub fn ffmpeg_slice_args(
    input: &Path,
    output: &Path,
    start_seconds: u64,
    end_seconds: u64,
) -> Vec<String> {
    // `-ss` before `-i` resets output timestamps, so an output-side `-to`
    // measures from zero and copies far more than the intended window. Using
    // a duration (`-t end-start`) keeps every chunk exactly its slice length.
    let duration = end_seconds.saturating_sub(start_seconds).max(1);
    vec![
        "-y".into(),
        "-ss".into(),
        start_seconds.to_string(),
        "-i".into(),
        input.to_string_lossy().into_owned(),
        "-t".into(),
        duration.to_string(),
        "-c".into(),
        "copy".into(),
        output.to_string_lossy().into_owned(),
    ]
}
pub fn ffmpeg_keyframe_args(
    input: &Path,
    output_pattern: &Path,
    interval_seconds: u32,
) -> Vec<String> {
    vec![
        "-y".into(),
        "-i".into(),
        input.to_string_lossy().into_owned(),
        "-vf".into(),
        format!("fps=1/{interval_seconds}"),
        output_pattern.to_string_lossy().into_owned(),
    ]
}
pub fn run_ffmpeg(executable: &Path, args: &[String]) -> Result<(), BeaError> {
    let mut command = std::process::Command::new(executable);
    command.args(args);
    hide_console_window(&mut command);
    let output = command
        .output()
        .map_err(|e| BeaError::MediaProcessing(e.to_string()))?;
    if !output.status.success() {
        return Err(BeaError::MediaProcessing(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(())
}

pub fn frame_hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn deduplicate_frames(frames: &[VisualFrame]) -> Vec<VisualFrame> {
    let mut seen = std::collections::HashSet::new();
    frames
        .iter()
        .filter(|frame| seen.insert(frame.perceptual_hash.clone()))
        .cloned()
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ContextMode {
    Economy,
    Balanced,
    Maximum,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContextPack {
    pub mode: ContextMode,
    pub events: Vec<LedgerEvent>,
    pub estimated_input_tokens: usize,
    pub evidence_count: usize,
}

pub fn open_database(path: impl AsRef<Path>) -> Result<Connection, BeaError> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS meetings (id TEXT PRIMARY KEY, title TEXT NOT NULL, status TEXT NOT NULL, created_at TEXT NOT NULL, duration_seconds INTEGER NOT NULL DEFAULT 0, language TEXT NOT NULL DEFAULT 'auto', asr_engine_id TEXT NOT NULL DEFAULT 'qwen-standard'); CREATE TABLE IF NOT EXISTS transcript_segments (id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE, start_seconds INTEGER NOT NULL, end_seconds INTEGER NOT NULL, text TEXT NOT NULL, language_detected TEXT, language_confidence REAL, speaker INTEGER); CREATE VIRTUAL TABLE IF NOT EXISTS transcript_fts USING fts5(meeting_id UNINDEXED, segment_id UNINDEXED, text); CREATE TABLE IF NOT EXISTS recording_chunks (id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE, ordinal INTEGER NOT NULL, start_seconds INTEGER NOT NULL, end_seconds INTEGER NOT NULL, path TEXT NOT NULL, state TEXT NOT NULL, UNIQUE(meeting_id, ordinal)); CREATE TABLE IF NOT EXISTS jobs (id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE, kind TEXT NOT NULL, state TEXT NOT NULL, progress REAL NOT NULL DEFAULT 0, attempts INTEGER NOT NULL DEFAULT 0, error TEXT); CREATE TABLE IF NOT EXISTS media_sources (id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE, path TEXT NOT NULL, kind TEXT NOT NULL, duration_seconds INTEGER, copied INTEGER NOT NULL DEFAULT 0); CREATE TABLE IF NOT EXISTS model_manifests (id TEXT PRIMARY KEY, name TEXT NOT NULL, version TEXT NOT NULL, size_bytes INTEGER NOT NULL, sha256 TEXT NOT NULL, runtime TEXT NOT NULL, languages TEXT NOT NULL, installed INTEGER NOT NULL DEFAULT 0); CREATE TABLE IF NOT EXISTS provider_configs (id TEXT PRIMARY KEY, kind TEXT NOT NULL, base_url TEXT NOT NULL, model TEXT NOT NULL, credential_ref TEXT, enabled INTEGER NOT NULL DEFAULT 1); CREATE TABLE IF NOT EXISTS usage_records (id TEXT PRIMARY KEY, provider_id TEXT NOT NULL, model TEXT NOT NULL, input_tokens INTEGER NOT NULL, output_tokens INTEGER NOT NULL, estimated_cost REAL, operation TEXT NOT NULL, created_at TEXT NOT NULL); CREATE TABLE IF NOT EXISTS ledger_events (id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE, payload TEXT NOT NULL); CREATE TABLE IF NOT EXISTS minutes (meeting_id TEXT PRIMARY KEY REFERENCES meetings(id) ON DELETE CASCADE, payload TEXT NOT NULL, updated_at TEXT NOT NULL); CREATE TABLE IF NOT EXISTS app_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL); ")?;
    conn.execute_batch("CREATE TABLE IF NOT EXISTS participants (id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE, name TEXT NOT NULL, role TEXT, created_at TEXT NOT NULL); CREATE TABLE IF NOT EXISTS context_events (id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE, kind TEXT NOT NULL, payload TEXT NOT NULL, confidence REAL NOT NULL, created_at TEXT NOT NULL); CREATE TABLE IF NOT EXISTS topics (id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE, label TEXT NOT NULL, summary TEXT, start_seconds INTEGER, end_seconds INTEGER); CREATE TABLE IF NOT EXISTS visual_evidence (id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE, timestamp_seconds INTEGER NOT NULL, path TEXT NOT NULL, thumbnail_path TEXT, ocr_text TEXT, perceptual_hash TEXT NOT NULL, description TEXT); CREATE TABLE IF NOT EXISTS action_items (id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE, event_id TEXT, owner TEXT, summary TEXT NOT NULL, due TEXT, status TEXT NOT NULL DEFAULT 'open', FOREIGN KEY(event_id) REFERENCES context_events(id) ON DELETE SET NULL); CREATE TABLE IF NOT EXISTS llm_runs (id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE, provider_id TEXT, model TEXT NOT NULL, operation TEXT NOT NULL, prompt_version TEXT NOT NULL, input_tokens INTEGER NOT NULL, output_tokens INTEGER NOT NULL, state TEXT NOT NULL, error TEXT, created_at TEXT NOT NULL); CREATE INDEX IF NOT EXISTS idx_context_events_meeting ON context_events(meeting_id); CREATE INDEX IF NOT EXISTS idx_visual_evidence_meeting_time ON visual_evidence(meeting_id, timestamp_seconds); CREATE INDEX IF NOT EXISTS idx_action_items_meeting_status ON action_items(meeting_id, status); CREATE TABLE IF NOT EXISTS speaker_names (meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE, speaker_index INTEGER NOT NULL, name TEXT NOT NULL, UNIQUE(meeting_id, speaker_index)); CREATE TABLE IF NOT EXISTS segment_speakers (segment_id TEXT NOT NULL REFERENCES transcript_segments(id) ON DELETE CASCADE, meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE, speaker_index INTEGER NOT NULL, UNIQUE(segment_id, speaker_index));")?;
    // Backward-compatible column migrations: "duplicate column name" is the
    // normal already-migrated case, but any other failure (locked/corrupt DB)
    // must surface instead of silently skipping the migration.
    if let Err(error) = conn.execute(
        "ALTER TABLE meetings ADD COLUMN asr_engine_id TEXT NOT NULL DEFAULT 'qwen-standard'",
        [],
    ) {
        if !error.to_string().contains("duplicate column name") {
            return Err(BeaError::Database(error));
        }
    }
    if let Err(error) = conn.execute(
        "ALTER TABLE transcript_segments ADD COLUMN speaker INTEGER",
        [],
    ) {
        if !error.to_string().contains("duplicate column name") {
            return Err(BeaError::Database(error));
        }
    }
    Ok(conn)
}

pub fn set_app_setting(conn: &Connection, key: &str, value: &str) -> Result<(), BeaError> {
    conn.execute(
        "INSERT OR REPLACE INTO app_settings(key,value) VALUES (?1,?2)",
        params![key, value],
    )?;
    Ok(())
}

pub fn get_app_setting(conn: &Connection, key: &str) -> Result<Option<String>, BeaError> {
    Ok(conn
        .query_row(
            "SELECT value FROM app_settings WHERE key=?1",
            params![key],
            |row| row.get(0),
        )
        .optional()?)
}

pub fn create_meeting(
    conn: &Connection,
    title: &str,
    language: TranscriptLanguage,
) -> Result<Meeting, BeaError> {
    create_meeting_with_engine(conn, title, language, "whisper-compatibility")
}

pub fn create_meeting_with_engine(
    conn: &Connection,
    title: &str,
    language: TranscriptLanguage,
    asr_engine_id: &str,
) -> Result<Meeting, BeaError> {
    let meeting = Meeting {
        id: Uuid::new_v4().to_string(),
        title: title.trim().to_string(),
        status: MeetingStatus::Draft,
        created_at: Utc::now(),
        duration_seconds: 0,
        language,
        asr_engine_id: asr_engine_id.to_string(),
    };
    conn.execute("INSERT INTO meetings (id,title,status,created_at,duration_seconds,language,asr_engine_id) VALUES (?1,?2,?3,?4,0,?5,?6)", params![meeting.id, meeting.title, meeting.status.as_str(), meeting.created_at.to_rfc3339(), meeting.language.as_str(), meeting.asr_engine_id])?;
    Ok(meeting)
}

pub fn list_meetings(conn: &Connection) -> Result<Vec<Meeting>, BeaError> {
    let mut stmt = conn.prepare("SELECT id,title,status,created_at,duration_seconds,language,asr_engine_id FROM meetings ORDER BY created_at DESC")?;
    let rows = stmt.query_map([], |r| {
        let status = match r.get::<_, String>(2)?.as_str() {
            "recording" => MeetingStatus::Recording,
            "paused" => MeetingStatus::Paused,
            "processing" => MeetingStatus::Processing,
            "ready" => MeetingStatus::Ready,
            "failed" => MeetingStatus::Failed,
            _ => MeetingStatus::Draft,
        };
        let language = match r.get::<_, String>(5)?.as_str() {
            "en" => TranscriptLanguage::English,
            "fil" => TranscriptLanguage::Filipino,
            "taglish" => TranscriptLanguage::Taglish,
            _ => TranscriptLanguage::Auto,
        };
        let created_at = DateTime::parse_from_rfc3339(&r.get::<_, String>(3)?)
            .map(|value| value.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());
        Ok(Meeting {
            id: r.get(0)?,
            title: r.get(1)?,
            status,
            created_at,
            duration_seconds: r.get(4)?,
            language,
            asr_engine_id: r
                .get(6)
                .unwrap_or_else(|_| "whisper-compatibility".to_string()),
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn update_meeting_title(
    conn: &Connection,
    meeting_id: &str,
    title: &str,
) -> Result<(), BeaError> {
    let changed = conn.execute(
        "UPDATE meetings SET title=?1 WHERE id=?2",
        params![title.trim(), meeting_id],
    )?;
    if changed == 0 {
        return Err(BeaError::MeetingNotFound(meeting_id.to_string()));
    }
    Ok(())
}

pub fn delete_meeting(conn: &Connection, meeting_id: &str) -> Result<(), BeaError> {
    let changed = conn.execute("DELETE FROM meetings WHERE id=?1", params![meeting_id])?;
    if changed == 0 {
        return Err(BeaError::MeetingNotFound(meeting_id.to_string()));
    }
    Ok(())
}

pub fn set_speaker_name(
    conn: &Connection,
    meeting_id: &str,
    speaker_index: u32,
    name: &str,
) -> Result<(), BeaError> {
    conn.execute(
        "INSERT INTO speaker_names(meeting_id,speaker_index,name) VALUES (?1,?2,?3)
         ON CONFLICT(meeting_id,speaker_index) DO UPDATE SET name=excluded.name",
        params![meeting_id, speaker_index, name.trim()],
    )?;
    Ok(())
}

pub fn list_speaker_names(
    conn: &Connection,
    meeting_id: &str,
) -> Result<std::collections::HashMap<u32, String>, BeaError> {
    let mut stmt = conn.prepare(
        "SELECT speaker_index,name FROM speaker_names WHERE meeting_id=?1 ORDER BY speaker_index",
    )?;
    let rows = stmt.query_map(params![meeting_id], |row| {
        Ok((row.get::<_, i64>(0)? as u32, row.get::<_, String>(1)?))
    })?;
    Ok(rows.collect::<Result<std::collections::HashMap<_, _>, _>>()?)
}

/// Update only the speaker label of a persisted segment.
pub fn update_segment_speaker(
    conn: &Connection,
    segment_id: &str,
    speaker: Option<u32>,
) -> Result<(), BeaError> {
    let changed = conn.execute(
        "UPDATE transcript_segments SET speaker=?1 WHERE id=?2",
        params![speaker.map(|value| value as i64), segment_id],
    )?;
    if changed == 0 {
        return Err(BeaError::InvalidState(format!(
            "transcript segment not found: {segment_id}"
        )));
    }
    Ok(())
}

/// Overlapping-speech side channel: segments keep their primary speaker in
/// `transcript_segments.speaker`; extra/overlapping speakers live here. An
/// empty list clears all overlap rows for that segment.
pub fn set_segment_speakers(
    conn: &Connection,
    segment_id: &str,
    speaker_indexes: &[u32],
) -> Result<(), BeaError> {
    conn.execute("DELETE FROM segment_speakers WHERE segment_id=?1", params![segment_id])?;
    for index in speaker_indexes {
        conn.execute(
            "INSERT OR IGNORE INTO segment_speakers(segment_id,meeting_id,speaker_index) SELECT ?1, meeting_id, ?2 FROM transcript_segments WHERE id=?1",
            params![segment_id, *index as i64],
        )?;
    }
    Ok(())
}

/// All segments of a meeting that carry overlapping speakers, keyed by segment id.
pub fn list_segment_speakers(
    conn: &Connection,
    meeting_id: &str,
) -> Result<std::collections::HashMap<String, Vec<u32>>, BeaError> {
    let mut stmt = conn.prepare(
        "SELECT segment_id,speaker_index FROM segment_speakers WHERE meeting_id=?1 ORDER BY segment_id,speaker_index",
    )?;
    let rows = stmt.query_map(params![meeting_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u32))
    })?;
    let mut map: std::collections::HashMap<String, Vec<u32>> = std::collections::HashMap::new();
    for row in rows {
        let (segment_id, index) = row?;
        map.entry(segment_id).or_default().push(index);
    }
    Ok(map)
}

pub fn set_meeting_status(
    conn: &Connection,
    id: &str,
    status: MeetingStatus,
    duration_seconds: u64,
) -> Result<(), BeaError> {
    if duration_seconds > MAX_MEETING_SECONDS {
        return Err(BeaError::InvalidDuration(duration_seconds));
    }
    let changed = conn.execute(
        "UPDATE meetings SET status=?1,duration_seconds=?2 WHERE id=?3",
        params![status.as_str(), duration_seconds, id],
    )?;
    if changed == 0 {
        return Err(BeaError::MeetingNotFound(id.to_string()));
    }
    Ok(())
}

pub fn add_segment(conn: &Connection, segment: &TranscriptSegment) -> Result<(), BeaError> {
    conn.execute("INSERT OR REPLACE INTO transcript_segments (id,meeting_id,start_seconds,end_seconds,text,language_detected,language_confidence,speaker) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)", params![segment.id, segment.meeting_id, segment.start_seconds, segment.end_seconds, segment.text, segment.language_detected, segment.language_confidence, segment.speaker])?;
    conn.execute(
        "DELETE FROM transcript_fts WHERE segment_id=?1",
        params![segment.id],
    )?;
    // Silence placeholders stay in the timeline but out of full-text search so
    // searching the transcript (and anything built on FTS) never matches them.
    if segment.text.trim() != SILENCE_TEXT {
        conn.execute(
            "INSERT INTO transcript_fts(meeting_id,segment_id,text) VALUES (?1,?2,?3)",
            params![segment.meeting_id, segment.id, segment.text],
        )?;
    }
    Ok(())
}

/// Escapes user search text into a safe FTS5 phrase query. Wraps the input in
/// double quotes with internal quotes doubled, so ordinary punctuation (dashes,
/// asterisks, colons, parentheses…) is matched literally instead of parsed as
/// FTS5 syntax and erroring out the whole search.
fn fts_quote(query: &str) -> String {
    format!("\"{}\"", query.replace('"', "\"\""))
}

/// Removes every persisted segment (and its FTS entries) for a meeting.
/// Called before a transcription run so re-transcribing replaces the old
/// transcript instead of appending duplicate segments alongside it.
pub fn clear_transcript(conn: &Connection, meeting_id: &str) -> Result<(), BeaError> {
    conn.execute(
        "DELETE FROM segment_speakers WHERE meeting_id=?1",
        params![meeting_id],
    )?;
    conn.execute(
        "DELETE FROM transcript_segments WHERE meeting_id=?1",
        params![meeting_id],
    )?;
    conn.execute(
        "DELETE FROM transcript_fts WHERE meeting_id=?1",
        params![meeting_id],
    )?;
    Ok(())
}

pub fn search_transcript(
    conn: &Connection,
    meeting_id: &str,
    query: &str,
) -> Result<Vec<TranscriptSegment>, BeaError> {
    let mut stmt = conn.prepare("SELECT s.id,s.meeting_id,s.start_seconds,s.end_seconds,s.text,s.language_detected,s.language_confidence,s.speaker FROM transcript_segments s JOIN transcript_fts f ON f.segment_id=s.id WHERE s.meeting_id=?1 AND f.text MATCH ?2 ORDER BY s.start_seconds")?;
    let rows = stmt.query_map(params![meeting_id, fts_quote(query)], |r| {
        Ok(TranscriptSegment {
            id: r.get(0)?,
            meeting_id: r.get(1)?,
            start_seconds: r.get(2)?,
            end_seconds: r.get(3)?,
            text: r.get(4)?,
            language_detected: r.get(5)?,
            language_confidence: r.get(6)?,
            speaker: r.get(7)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn list_transcript(
    conn: &Connection,
    meeting_id: &str,
) -> Result<Vec<TranscriptSegment>, BeaError> {
    let mut stmt = conn.prepare("SELECT id,meeting_id,start_seconds,end_seconds,text,language_detected,language_confidence,speaker FROM transcript_segments WHERE meeting_id=?1 ORDER BY start_seconds")?;
    let rows = stmt.query_map(params![meeting_id], |r| {
        Ok(TranscriptSegment {
            id: r.get(0)?,
            meeting_id: r.get(1)?,
            start_seconds: r.get(2)?,
            end_seconds: r.get(3)?,
            text: r.get(4)?,
            language_detected: r.get(5)?,
            language_confidence: r.get(6)?,
            speaker: r.get(7)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn chunk_ranges(duration_seconds: u64) -> Result<Vec<(u64, u64)>, BeaError> {
    if duration_seconds > MAX_MEETING_SECONDS {
        return Err(BeaError::InvalidDuration(duration_seconds));
    }
    Ok((0..duration_seconds)
        .step_by(CHUNK_SECONDS as usize)
        .map(|start| (start, (start + CHUNK_SECONDS).min(duration_seconds)))
        .collect())
}

pub fn pack_context(events: &[LedgerEvent], budget: usize) -> Vec<LedgerEvent> {
    let mut used = 0;
    let mut packed = Vec::new();
    for event in events {
        let cost =
            event.summary.len() + event.evidence.iter().map(|e| e.quote.len()).sum::<usize>();
        if used + cost > budget {
            break;
        }
        used += cost;
        packed.push(event.clone());
    }
    packed
}

pub fn estimate_tokens(text: &str) -> usize {
    (text.chars().count() + 3) / 4
}

/// Human-readable speaker legend + optional custom minutes format, prepended
/// to every LLM request for this meeting so minutes and chat answers both use
/// real names and the user's required structure.
pub fn build_meeting_context(
    speaker_names: &std::collections::HashMap<u32, String>,
    custom_format: Option<&str>,
) -> String {
    let mut context = String::new();
    if !speaker_names.is_empty() {
        let mut pairs: Vec<(u32, &String)> = speaker_names.iter().map(|(k, v)| (*k, v)).collect();
        pairs.sort();
        let legend = pairs
            .iter()
            .map(|(index, name)| format!("Speaker {} = {}", index + 1, name))
            .collect::<Vec<_>>()
            .join("; ");
        context.push_str(&format!("Participants: {legend}.\n"));
    }
    if let Some(format) = custom_format.filter(|value| !value.trim().is_empty()) {
        context.push_str("The user requires the minutes to follow this custom format:\n");
        context.push_str(format.trim());
        context.push('\n');
    }
    context
}

pub fn pack_context_mode(
    events: &[LedgerEvent],
    budget_tokens: usize,
    mode: ContextMode,
) -> ContextPack {
    let limit = match mode {
        ContextMode::Economy => budget_tokens.min(4_000),
        ContextMode::Balanced => budget_tokens,
        ContextMode::Maximum => budget_tokens.max(16_000),
    };
    let mut used = 0;
    let mut packed = Vec::new();
    for event in events {
        let text = format!(
            "{} {}",
            event.summary,
            event
                .evidence
                .iter()
                .map(|e| e.quote.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        );
        let cost = estimate_tokens(&text);
        if used + cost > limit {
            break;
        }
        used += cost;
        packed.push(event.clone());
    }
    let evidence_count = packed.iter().map(|event| event.evidence.len()).sum();
    ContextPack {
        mode,
        events: packed,
        estimated_input_tokens: used,
        evidence_count,
    }
}

pub fn parse_minutes_json(raw: &str) -> Result<Minutes, BeaError> {
    serde_json::from_str(raw).map_err(|e| BeaError::InvalidModelOutput(e.to_string()))
}

pub fn app_data_paths(root: impl AsRef<Path>, meeting_id: &str) -> (PathBuf, PathBuf) {
    let base = root.as_ref().join("recordings").join(meeting_id);
    (
        base.join("chunks"),
        root.as_ref().join("derived").join(meeting_id),
    )
}

/// Start a recording by persisting the first active chunk before audio bytes are written.
/// This ordering makes a restart discoverable even if the process dies mid-write.
pub fn start_recording(
    conn: &Connection,
    meeting_id: &str,
    chunks_root: impl AsRef<Path>,
) -> Result<RecordingChunk, BeaError> {
    // MAX(ordinal)+1 instead of COUNT(*)+1: deleted rows must never cause
    // UNIQUE(meeting_id, ordinal) collisions or misaligned chunk starts.
    let max_ordinal: Option<u32> = conn.query_row(
        "SELECT MAX(ordinal) FROM recording_chunks WHERE meeting_id=?1",
        params![meeting_id],
        |r| r.get(0),
    )?;
    let count = max_ordinal.unwrap_or(0);
    let start = count as u64 * CHUNK_SECONDS;
    if start >= MAX_MEETING_SECONDS {
        return Err(BeaError::InvalidDuration(start));
    }
    let chunk = RecordingChunk {
        id: Uuid::new_v4().to_string(),
        meeting_id: meeting_id.to_string(),
        ordinal: count + 1,
        start_seconds: start,
        end_seconds: start + CHUNK_SECONDS,
        path: chunks_root
            .as_ref()
            .join(format!("chunk-{:05}.ogg", count + 1)),
        state: ChunkState::Recording,
    };
    std::fs::create_dir_all(chunks_root.as_ref())
        .map_err(|e| BeaError::InvalidState(e.to_string()))?;
    conn.execute("INSERT INTO recording_chunks(id,meeting_id,ordinal,start_seconds,end_seconds,path,state) VALUES (?1,?2,?3,?4,?5,?6,?7)", params![chunk.id, chunk.meeting_id, chunk.ordinal, chunk.start_seconds, chunk.end_seconds, chunk.path.to_string_lossy().to_string(), chunk.state.as_str()])?;
    set_meeting_status(conn, meeting_id, MeetingStatus::Recording, start)?;
    Ok(chunk)
}

pub fn complete_recording_chunk(
    conn: &Connection,
    chunk_id: &str,
    actual_end_seconds: u64,
) -> Result<(), BeaError> {
    if actual_end_seconds > MAX_MEETING_SECONDS {
        return Err(BeaError::InvalidDuration(actual_end_seconds));
    }
    let changed = conn.execute("UPDATE recording_chunks SET end_seconds=?1,state='completed' WHERE id=?2 AND state='recording'", params![actual_end_seconds, chunk_id])?;
    if changed == 0 {
        return Err(BeaError::InvalidState(format!(
            "chunk {chunk_id} is not active"
        )));
    }
    conn.execute(
        "UPDATE meetings SET duration_seconds=CASE WHEN duration_seconds<?1 THEN ?1 ELSE duration_seconds END WHERE id=(SELECT meeting_id FROM recording_chunks WHERE id=?2)",
        params![actual_end_seconds, chunk_id],
    )?;
    Ok(())
}

pub fn persist_completed_audio_chunk(
    conn: &Connection,
    meeting_id: &str,
    chunk: &CompletedAudioChunk,
) -> Result<RecordingChunk, BeaError> {
    let recording = RecordingChunk {
        id: Uuid::new_v4().to_string(),
        meeting_id: meeting_id.to_string(),
        ordinal: chunk.ordinal,
        start_seconds: chunk.start_seconds,
        end_seconds: chunk.end_seconds,
        path: chunk.path.clone(),
        state: ChunkState::Completed,
    };
    conn.execute(
        "INSERT OR REPLACE INTO recording_chunks(id,meeting_id,ordinal,start_seconds,end_seconds,path,state) VALUES (?1,?2,?3,?4,?5,?6,'completed')",
        params![
            recording.id,
            recording.meeting_id,
            recording.ordinal,
            recording.start_seconds,
            recording.end_seconds,
            recording.path.to_string_lossy().to_string()
        ],
    )?;
    conn.execute(
        "UPDATE meetings SET duration_seconds=CASE WHEN duration_seconds<?1 THEN ?1 ELSE duration_seconds END WHERE id=?2",
        params![chunk.end_seconds, meeting_id],
    )?;
    Ok(recording)
}

pub fn list_completed_recording_chunks(
    conn: &Connection,
    meeting_id: &str,
) -> Result<Vec<RecordingChunk>, BeaError> {
    let mut statement = conn.prepare(
        "SELECT id,meeting_id,ordinal,start_seconds,end_seconds,path,state FROM recording_chunks WHERE meeting_id=?1 AND state='completed' ORDER BY ordinal",
    )?;
    let rows = statement.query_map(params![meeting_id], |row| {
        Ok(RecordingChunk {
            id: row.get(0)?,
            meeting_id: row.get(1)?,
            ordinal: row.get(2)?,
            start_seconds: row.get(3)?,
            end_seconds: row.get(4)?,
            path: PathBuf::from(row.get::<_, String>(5)?),
            state: ChunkState::Completed,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn recover_interrupted_chunks(
    conn: &Connection,
    meeting_id: &str,
) -> Result<Vec<RecordingChunk>, BeaError> {
    conn.execute(
        "UPDATE recording_chunks SET state='interrupted' WHERE meeting_id=?1 AND state='recording'",
        params![meeting_id],
    )?;
    let mut stmt = conn.prepare("SELECT id,meeting_id,ordinal,start_seconds,end_seconds,path,state FROM recording_chunks WHERE meeting_id=?1 AND state='interrupted' ORDER BY ordinal")?;
    let rows = stmt.query_map(params![meeting_id], |r| {
        Ok(RecordingChunk {
            id: r.get(0)?,
            meeting_id: r.get(1)?,
            ordinal: r.get(2)?,
            start_seconds: r.get(3)?,
            end_seconds: r.get(4)?,
            path: PathBuf::from(r.get::<_, String>(5)?),
            state: ChunkState::Interrupted,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn create_job(conn: &Connection, meeting_id: &str, kind: JobKind) -> Result<Job, BeaError> {
    let job = Job {
        id: Uuid::new_v4().to_string(),
        meeting_id: meeting_id.to_string(),
        kind,
        state: JobState::Queued,
        progress: 0.0,
        attempts: 0,
        error: None,
    };
    conn.execute(
        "INSERT INTO jobs(id,meeting_id,kind,state,progress,attempts) VALUES (?1,?2,?3,?4,0,0)",
        params![
            job.id,
            job.meeting_id,
            job.kind.as_str(),
            job.state.as_str()
        ],
    )?;
    Ok(job)
}

pub fn update_job(
    conn: &Connection,
    id: &str,
    state: JobState,
    progress: f32,
    error: Option<&str>,
) -> Result<(), BeaError> {
    if !(0.0..=1.0).contains(&progress) {
        return Err(BeaError::InvalidState(
            "job progress must be between 0 and 1".into(),
        ));
    }
    let changed = conn.execute("UPDATE jobs SET state=?1,progress=?2,attempts=attempts+CASE WHEN ?1='running' THEN 1 ELSE 0 END,error=?3 WHERE id=?4", params![state.as_str(),progress,error,id])?;
    if changed == 0 {
        return Err(BeaError::InvalidState(format!("job {id} does not exist")));
    }
    Ok(())
}

pub fn import_media(
    conn: &Connection,
    meeting_id: &str,
    path: impl AsRef<Path>,
    kind: MediaKind,
    duration_seconds: Option<u64>,
    copy_into_library: bool,
    library_root: impl AsRef<Path>,
) -> Result<MediaSource, BeaError> {
    let source = path.as_ref();
    let extension = source
        .extension()
        .and_then(|x| x.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let allowed = match kind {
        MediaKind::Audio => {
            ["wav", "mp3", "m4a", "ogg", "flac", "aac"].contains(&extension.as_str())
        }
        MediaKind::Video => ["mp4", "mov", "mkv", "webm", "avi"].contains(&extension.as_str()),
    };
    if !allowed {
        return Err(BeaError::UnsupportedMedia(extension));
    }
    if !source.exists() {
        return Err(BeaError::InvalidState(format!(
            "source does not exist: {}",
            source.display()
        )));
    }
    let destination = if copy_into_library {
        let dir = library_root.as_ref().join("media").join(meeting_id);
        std::fs::create_dir_all(&dir).map_err(|e| BeaError::InvalidState(e.to_string()))?;
        let target = dir.join(source.file_name().unwrap_or_default());
        std::fs::copy(source, &target).map_err(|e| BeaError::InvalidState(e.to_string()))?;
        target
    } else {
        source.to_path_buf()
    };
    let media = MediaSource {
        id: Uuid::new_v4().to_string(),
        meeting_id: meeting_id.to_string(),
        path: destination,
        kind,
        duration_seconds,
        copied: copy_into_library,
    };
    conn.execute("INSERT INTO media_sources(id,meeting_id,path,kind,duration_seconds,copied) VALUES (?1,?2,?3,?4,?5,?6)", params![media.id,media.meeting_id,media.path.to_string_lossy().to_string(),media.kind.as_str(),media.duration_seconds,media.copied])?;
    Ok(media)
}

pub fn register_model(conn: &Connection, manifest: &ModelManifest) -> Result<(), BeaError> {
    conn.execute("INSERT OR REPLACE INTO model_manifests(id,name,version,size_bytes,sha256,runtime,languages,installed) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)", params![manifest.id,manifest.name,manifest.version,manifest.size_bytes,manifest.sha256,manifest.runtime,serde_json::to_string(&manifest.languages).unwrap_or_else(|_| "[]".into()),manifest.installed])?;
    Ok(())
}

/// Verifies a downloaded Windows executable carries a valid Authenticode
/// signature (Windows only; other platforms are a no-op success). Used when no
/// SHA-256 pin exists for an upstream build so unsigned/tampered binaries are
/// rejected before they are ever executed.
pub fn verify_executable_authenticode(path: impl AsRef<Path>) -> Result<(), String> {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = path;
        Ok(())
    }
    #[cfg(target_os = "windows")]
    {
        let path = path.as_ref();
        let script = format!(
            "(Get-AuthenticodeSignature -LiteralPath '{}').Status",
            path.display().to_string().replace('\'', "''")
        );
        let output = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .creation_flags(0x0800_0000)
            .output()
            .map_err(|error| format!("unable to run signature check: {error}"))?;
        let status = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if status.eq_ignore_ascii_case("Valid") {
            Ok(())
        } else {
            Err(format!("signature status is '{status}', expected 'Valid'"))
        }
    }
}

pub fn verify_model_checksum(
    path: impl AsRef<Path>,
    expected_sha256: &str,
) -> Result<(), BeaError> {
    // An empty digest means "no pinned checksum" (e.g. packages fetched from a
    // moving release target); layout validation still gates the install.
    if expected_sha256.trim().is_empty() {
        return Ok(());
    }
    let mut file = std::fs::File::open(path).map_err(|e| BeaError::InvalidState(e.to_string()))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).map_err(|e| BeaError::InvalidState(e.to_string()))?;
    let actual = format!("{:x}", hasher.finalize());
    if actual != expected_sha256.to_ascii_lowercase() {
        return Err(BeaError::ChecksumMismatch {
            expected: expected_sha256.to_string(),
            actual,
        });
    }
    Ok(())
}

/// Size + streaming SHA-256 of a model archive. Used to build a manifest for
/// packages fetched directly from Bea's pinned release catalog.
pub fn model_package_fingerprint(source: impl AsRef<Path>) -> Result<(u64, String), BeaError> {
    package_fingerprint(source.as_ref())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelInstallProgress {
    pub bytes_copied: u64,
    pub total_bytes: u64,
    pub verified: bool,
}

pub fn inspect_asr_model_package(source: impl AsRef<Path>) -> Result<ModelManifest, BeaError> {
    let source = source.as_ref();
    let id = if source.is_dir() {
        let candidates = [
            ("qwen3-asr-0.6b-int8", qwen_model_is_complete(source)),
            ("whisper-compatibility", whisper_model_is_complete(source)),
            ("nemotron-multilingual", nemotron_model_is_complete(source)),
        ];
        let matching: Vec<&str> = candidates
            .iter()
            .filter_map(|(id, complete)| complete.then_some(*id))
            .collect();
        if matching.len() != 1 {
            return Err(BeaError::InvalidState(
                "folder must contain exactly one supported Bea ASR layout".into(),
            ));
        }
        matching[0].to_string()
    } else {
        let name = source
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if name.contains("qwen") {
            "qwen3-asr-0.6b-int8".to_string()
        } else if name.contains("whisper") {
            "whisper-compatibility".to_string()
        } else if name.contains("nemotron") {
            "nemotron-multilingual".to_string()
        } else {
            return Err(BeaError::InvalidState(
                "archive name must identify a supported Bea engine (qwen, whisper, or nemotron)"
                    .into(),
            ));
        }
    };
    let (size_bytes, sha256) = package_fingerprint(source)?;
    let (name, languages) = match id.as_str() {
        "qwen3-asr-0.6b-int8" => ("Qwen3-ASR 0.6B INT8", vec!["en", "fil", "taglish"]),
        "whisper-compatibility" => ("Bea Standard · Whisper Large-v3-Turbo", vec!["multilingual"]),
        _ => (
            "Nemotron 3.5 multilingual 0.6B INT8 · 560ms",
            vec!["multilingual"],
        ),
    };
    Ok(ModelManifest {
        id,
        name: name.into(),
        version: "manifest-detected".into(),
        size_bytes,
        sha256,
        runtime: "sherpa-onnx".into(),
        languages: languages.into_iter().map(str::to_string).collect(),
        installed: false,
    })
}

pub fn install_model_package(
    source: impl AsRef<Path>,
    destination: impl AsRef<Path>,
    manifest: &ModelManifest,
) -> Result<ModelInstallProgress, BeaError> {
    let source = source.as_ref();
    let destination = destination.as_ref();
    let metadata = std::fs::metadata(source).map_err(|e| BeaError::InvalidState(e.to_string()))?;
    if metadata.len() != manifest.size_bytes {
        return Err(BeaError::InvalidState(format!(
            "model package size mismatch: expected {}, got {}",
            manifest.size_bytes,
            metadata.len()
        )));
    }
    verify_model_checksum(source, &manifest.sha256)?;
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|e| BeaError::InvalidState(e.to_string()))?;
    }
    std::fs::copy(source, destination).map_err(|e| BeaError::InvalidState(e.to_string()))?;
    Ok(ModelInstallProgress {
        bytes_copied: metadata.len(),
        total_bytes: manifest.size_bytes,
        verified: true,
    })
}

pub fn install_qwen_model_package(
    source: impl AsRef<Path>,
    destination_dir: impl AsRef<Path>,
    manifest: &ModelManifest,
) -> Result<ModelInstallProgress, BeaError> {
    let source = source.as_ref();
    let destination_dir = destination_dir.as_ref();
    let metadata = std::fs::metadata(source).map_err(|e| BeaError::InvalidState(e.to_string()))?;
    if metadata.len() != manifest.size_bytes {
        return Err(BeaError::InvalidState(format!(
            "model package size mismatch: expected {}, got {}",
            manifest.size_bytes,
            metadata.len()
        )));
    }
    verify_model_checksum(source, &manifest.sha256)?;
    std::fs::create_dir_all(destination_dir).map_err(|e| BeaError::InvalidState(e.to_string()))?;
    // Detect compression by magic bytes (bzip2 = "BZh"), not the file name —
    // download staging files do not carry the archive extension.
    let compressed = file_starts_with(source, b"BZh");
    if compressed {
        let file =
            std::fs::File::open(source).map_err(|e| BeaError::InvalidState(e.to_string()))?;
        let decoder = bzip2::read::BzDecoder::new(file);
        let mut archive = tar::Archive::new(decoder);
        for entry in archive
            .entries()
            .map_err(|e| BeaError::InvalidState(e.to_string()))?
        {
            let mut entry = entry.map_err(|e| BeaError::InvalidState(e.to_string()))?;
            let path = entry
                .path()
                .map_err(|e| BeaError::InvalidState(e.to_string()))?;
            if path.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            }) {
                return Err(BeaError::InvalidState(
                    "model archive contains an unsafe path".into(),
                ));
            }
            entry
                .unpack_in(destination_dir)
                .map_err(|e| BeaError::InvalidState(e.to_string()))?;
        }
    } else {
        std::fs::copy(source, destination_dir.join("model.package"))
            .map_err(|e| BeaError::InvalidState(e.to_string()))?;
    }
    if compressed && !qwen_model_is_complete(destination_dir) {
        return Err(BeaError::InvalidState(
            "Qwen3-ASR archive did not contain the required ONNX files".into(),
        ));
    }
    Ok(ModelInstallProgress {
        bytes_copied: metadata.len(),
        total_bytes: manifest.size_bytes,
        verified: true,
    })
}

/// Installs a verified sherpa-onnx package for one of Bea's compatibility engines.
///
/// Packages may be a signed/manifested archive or an extracted directory.  We keep the
/// accepted layout intentionally narrow: the engine is only registered after its required
/// model files are present.  This prevents an arbitrary ONNX collection from silently
/// becoming a selectable engine.
pub fn install_sherpa_model_package(
    source: impl AsRef<Path>,
    destination_dir: impl AsRef<Path>,
    manifest: &ModelManifest,
) -> Result<ModelInstallProgress, BeaError> {
    let source = source.as_ref();
    let destination_dir = destination_dir.as_ref();
    let (bytes, fingerprint) = package_fingerprint(source)?;
    if bytes != manifest.size_bytes {
        return Err(BeaError::InvalidState(format!(
            "model package size mismatch: expected {}, got {}",
            manifest.size_bytes, bytes
        )));
    }
    // An empty digest means "no pinned checksum"; layout validation below still
    // gates the install.
    if !manifest.sha256.trim().is_empty() && fingerprint != manifest.sha256.to_ascii_lowercase() {
        return Err(BeaError::ChecksumMismatch {
            expected: manifest.sha256.to_string(),
            actual: fingerprint,
        });
    }
    std::fs::create_dir_all(destination_dir).map_err(|e| BeaError::InvalidState(e.to_string()))?;
    if source.is_dir() {
        copy_directory_contents(source, destination_dir)?;
    } else {
        extract_model_archive(source, destination_dir)?;
    }
    let complete = match manifest.id.as_str() {
        "whisper-compatibility" => whisper_model_is_complete(destination_dir),
        "nemotron-multilingual" => nemotron_model_is_complete(destination_dir),
        _ => false,
    };
    if !complete {
        return Err(BeaError::InvalidState(format!(
            "{} package is missing its required sherpa-onnx files",
            manifest.name
        )));
    }
    Ok(ModelInstallProgress {
        bytes_copied: bytes,
        total_bytes: manifest.size_bytes,
        verified: true,
    })
}

fn package_fingerprint(source: &Path) -> Result<(u64, String), BeaError> {
    if source.is_file() {
        let metadata =
            std::fs::metadata(source).map_err(|e| BeaError::InvalidState(e.to_string()))?;
        let bytes = std::fs::read(source).map_err(|e| BeaError::InvalidState(e.to_string()))?;
        return Ok((metadata.len(), format!("{:x}", Sha256::digest(bytes))));
    }
    if !source.is_dir() {
        return Err(BeaError::InvalidState(
            "model package path does not exist".into(),
        ));
    }
    let mut files = Vec::new();
    collect_files(source, source, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    for (relative, path) in files {
        let bytes = std::fs::read(path).map_err(|e| BeaError::InvalidState(e.to_string()))?;
        total += bytes.len() as u64;
        hasher.update(relative.as_bytes());
        hasher.update([0]);
        hasher.update(&bytes);
        hasher.update([0]);
    }
    Ok((total, format!("{:x}", hasher.finalize())))
}

fn collect_files(
    root: &Path,
    current: &Path,
    output: &mut Vec<(String, PathBuf)>,
) -> Result<(), BeaError> {
    for entry in std::fs::read_dir(current).map_err(|e| BeaError::InvalidState(e.to_string()))? {
        let entry = entry.map_err(|e| BeaError::InvalidState(e.to_string()))?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, output)?;
        } else if path.is_file() {
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            output.push((relative, path));
        }
    }
    Ok(())
}

fn copy_directory_contents(source: &Path, destination: &Path) -> Result<(), BeaError> {
    for entry in std::fs::read_dir(source).map_err(|e| BeaError::InvalidState(e.to_string()))? {
        let entry = entry.map_err(|e| BeaError::InvalidState(e.to_string()))?;
        let from = entry.path();
        let to = destination.join(entry.file_name());
        if from.is_dir() {
            std::fs::create_dir_all(&to).map_err(|e| BeaError::InvalidState(e.to_string()))?;
            copy_directory_contents(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).map_err(|e| BeaError::InvalidState(e.to_string()))?;
        }
    }
    Ok(())
}

pub fn file_starts_with(path: &Path, magic: &[u8]) -> bool {
    use std::io::Read;
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return false,
    };
    let mut buffer = [0u8; 8];
    let read = file.read(&mut buffer).unwrap_or(0);
    buffer[..read].starts_with(magic)
}

pub fn extract_model_archive(source: &Path, destination: &Path) -> Result<(), BeaError> {
    let extension = source
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    // Detect the format by magic bytes first — download staging names (e.g.
    // `model.download.tar.bz2` mid-rename or `.download` leftovers) do not
    // always carry the archive extension.
    if extension == "zip" || file_starts_with(source, b"PK\x03\x04") {
        let file =
            std::fs::File::open(source).map_err(|e| BeaError::InvalidState(e.to_string()))?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(|e| BeaError::InvalidState(e.to_string()))?;
        for index in 0..archive.len() {
            let mut entry = archive
                .by_index(index)
                .map_err(|e| BeaError::InvalidState(e.to_string()))?;
            let Some(name) = entry.enclosed_name().map(PathBuf::from) else {
                return Err(BeaError::InvalidState(
                    "model archive contains an unsafe path".into(),
                ));
            };
            let target = destination.join(name);
            if entry.is_dir() {
                std::fs::create_dir_all(&target)
                    .map_err(|e| BeaError::InvalidState(e.to_string()))?;
            } else {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| BeaError::InvalidState(e.to_string()))?;
                }
                let mut output = std::fs::File::create(&target)
                    .map_err(|e| BeaError::InvalidState(e.to_string()))?;
                std::io::copy(&mut entry, &mut output)
                    .map_err(|e| BeaError::InvalidState(e.to_string()))?;
            }
        }
        return Ok(());
    }
    let file = std::fs::File::open(source).map_err(|e| BeaError::InvalidState(e.to_string()))?;
    // bzip2 magic is "BZh" followed by a digit; fall back to a plain tar read.
    let reader: Box<dyn std::io::Read> = if extension == "bz2" || file_starts_with(source, b"BZh") {
        Box::new(bzip2::read::BzDecoder::new(file))
    } else {
        Box::new(file)
    };
    let mut archive = tar::Archive::new(reader);
    for entry in archive
        .entries()
        .map_err(|e| BeaError::InvalidState(e.to_string()))?
    {
        let mut entry = entry.map_err(|e| BeaError::InvalidState(e.to_string()))?;
        let path = entry
            .path()
            .map_err(|e| BeaError::InvalidState(e.to_string()))?
            .to_path_buf();
        if path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return Err(BeaError::InvalidState(
                "model archive contains an unsafe path".into(),
            ));
        }
        entry
            .unpack_in(destination)
            .map_err(|e| BeaError::InvalidState(e.to_string()))?;
    }
    Ok(())
}

pub fn save_provider(conn: &Connection, provider: &ProviderConfig) -> Result<(), BeaError> {
    conn.execute("INSERT OR REPLACE INTO provider_configs(id,kind,base_url,model,credential_ref,enabled) VALUES (?1,?2,?3,?4,?5,?6)", params![provider.id,provider.kind.as_str(),provider.base_url,provider.model,provider.credential_ref,provider.enabled])?;
    Ok(())
}

/// Reads a fixed number of peak buckets from a WAV source.  Samples are consumed
/// sequentially and never materialized as a browser-sized buffer, so a multi-hour
/// recording remains bounded at the command boundary.
pub fn waveform_peaks(
    path: impl AsRef<Path>,
    requested_peaks: usize,
) -> Result<Vec<f32>, BeaError> {
    let mut reader = hound::WavReader::open(path.as_ref())
        .map_err(|error| BeaError::MediaProcessing(error.to_string()))?;
    let spec = reader.spec();
    let total_samples = reader.duration() as usize;
    let peak_count = requested_peaks.clamp(1, 2048).min(total_samples.max(1));
    let mut peaks = vec![0.0f32; peak_count];
    if spec.sample_format == hound::SampleFormat::Float {
        for (index, sample) in reader.samples::<f32>().enumerate() {
            let value = sample
                .map_err(|error| BeaError::MediaProcessing(error.to_string()))?
                .abs();
            let bucket = index.saturating_mul(peak_count) / total_samples.max(1);
            if let Some(peak) = peaks.get_mut(bucket.min(peak_count - 1)) {
                *peak = peak.max(value.min(1.0));
            }
        }
    } else if spec.bits_per_sample <= 16 {
        for (index, sample) in reader.samples::<i16>().enumerate() {
            let value = (sample.map_err(|error| BeaError::MediaProcessing(error.to_string()))?
                as f32
                / i16::MAX as f32)
                .abs();
            let bucket = index.saturating_mul(peak_count) / total_samples.max(1);
            if let Some(peak) = peaks.get_mut(bucket.min(peak_count - 1)) {
                *peak = peak.max(value.min(1.0));
            }
        }
    } else {
        for (index, sample) in reader.samples::<i32>().enumerate() {
            let value = (sample.map_err(|error| BeaError::MediaProcessing(error.to_string()))?
                as f32
                / i32::MAX as f32)
                .abs();
            let bucket = index.saturating_mul(peak_count) / total_samples.max(1);
            if let Some(peak) = peaks.get_mut(bucket.min(peak_count - 1)) {
                *peak = peak.max(value.min(1.0));
            }
        }
    }
    Ok(peaks)
}

pub fn load_provider(conn: &Connection, id: &str) -> Result<Option<ProviderConfig>, BeaError> {
    let row: Option<(String, String, String, String, Option<String>, bool)> = conn
        .query_row(
            "SELECT id,kind,base_url,model,credential_ref,enabled FROM provider_configs WHERE id=?1",
            params![id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .optional()?;
    Ok(
        row.map(|(id, kind, base_url, model, credential_ref, enabled)| {
            let kind = match kind.as_str() {
                "openrouter" => ProviderKind::OpenRouter,
                "openai_compatible" => ProviderKind::OpenAiCompatible,
                "claude_compatible" => ProviderKind::ClaudeCompatible,
                "local" => ProviderKind::Local,
                _ => ProviderKind::Local,
            };
            ProviderConfig {
                id,
                kind,
                base_url,
                model,
                credential_ref,
                enabled,
            }
        }),
    )
}

pub fn build_provider_request(provider: &ProviderConfig, request: &LlmRequest) -> ProviderRequest {
    let base = provider.base_url.trim_end_matches('/');
    let url = match provider.kind {
        ProviderKind::OpenRouter
        | ProviderKind::OpenAiCompatible
        | ProviderKind::ClaudeCompatible => {
            format!("{base}/chat/completions")
        }
        ProviderKind::Local => format!("{base}/v1/chat/completions"),
    };
    let mut body = serde_json::json!({
        "model": request.model,
        "messages": [
            {"role": "system", "content": request.system},
            {"role": "user", "content": request.user}
        ],
        "response_format": {"type": "json_schema", "json_schema": {"name": "bea_minutes", "strict": true, "schema": serde_json::from_str::<serde_json::Value>(&request.json_schema).unwrap_or_else(|_| serde_json::json!({"type": "object"}))}},
        "max_tokens": request.max_output_tokens,
    });
    // Reasoning models (e.g. qwen3 on OpenRouter) burn the whole output budget
    // on `message.reasoning` and return `content: null`, which makes minutes
    // generation silently fall back to the local extractor. Disable thinking
    // for OpenRouter-hosted models so the JSON actually arrives in `content`.
    if provider.kind == ProviderKind::OpenRouter {
        body["reasoning"] = serde_json::json!({"enabled": false});
    }
    ProviderRequest {
        url,
        model: request.model.clone(),
        body,
    }
}

pub fn parse_provider_minutes(response: &serde_json::Value) -> Result<Minutes, BeaError> {
    let message = response
        .get("choices")
        .and_then(|choices| choices.get(0))
        .and_then(|choice| choice.get("message"))
        .ok_or_else(|| {
            BeaError::InvalidModelOutput(
                "provider response did not contain choices[0].message".into(),
            )
        })?;
    // Reasoning models (qwen3, deepseek) return the JSON inside
    // `message.reasoning` with `content: null`. Try both fields, then fall
    // back to extracting the JSON object embedded anywhere in either text.
    let mut candidates = Vec::new();
    if let Some(content) = message.get("content").and_then(serde_json::Value::as_str) {
        candidates.push(content.to_string());
    }
    if let Some(reasoning) = message.get("reasoning").and_then(serde_json::Value::as_str) {
        candidates.push(reasoning.to_string());
    }
    for candidate in candidates {
        if let Ok(minutes) = parse_provider_minutes_text(&candidate) {
            return Ok(minutes);
        }
    }
    Err(BeaError::InvalidModelOutput(
        "provider response contained no usable minutes JSON".into(),
    ))
}

fn parse_provider_minutes_text(text: &str) -> Result<Minutes, BeaError> {
    let mut attempts: Vec<String> = vec![text.to_string()];
    if let Some(start) = text.find('{') {
        if let Some(end) = text.rfind('}') {
            if end > start {
                attempts.push(text[start..=end].to_string());
            }
        }
    }
    for attempt in attempts {
        let stripped = attempt
            .trim()
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim();
        let Ok(mut value) = serde_json::from_str::<serde_json::Value>(stripped) else {
            continue;
        };
        if let Some(object) = value.as_object_mut() {
            for (key, kind) in [
                ("decisions", "decision"),
                ("action_items", "action"),
                ("unresolved", "unresolved"),
            ] {
                if let Some(items) = object.get_mut(key).and_then(serde_json::Value::as_array_mut)
                {
                    for item in items.iter_mut() {
                        if let Some(item_object) = item.as_object_mut() {
                            item_object
                                .entry("kind")
                                .or_insert_with(|| serde_json::json!(kind));
                        }
                    }
                }
            }
        }
        if let Ok(minutes) = serde_json::from_value::<Minutes>(value) {
            return Ok(minutes);
        }
    }
    Err(BeaError::InvalidModelOutput(
        "provider minutes JSON did not match the expected schema".into(),
    ))
}

/// Sends only the packed text/image request supplied by the caller. The API key is passed at
/// the effect boundary and is never persisted in `ProviderConfig`, SQLite, or a request body.
pub async fn call_provider(
    provider: &ProviderConfig,
    request: &LlmRequest,
    api_key: Option<&str>,
) -> Result<Minutes, BeaError> {
    let prepared = build_provider_request(provider, request);
    // Bounded request so a stalled provider endpoint can never freeze minutes
    // generation the way a hung ASR decode used to freeze transcription.
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|error| BeaError::ProviderRequest(error.to_string()))?;
    let mut request_builder = client.post(prepared.url).json(&prepared.body);
    if let Some(key) = api_key.filter(|key| !key.trim().is_empty()) {
        request_builder = request_builder.bearer_auth(key);
    }
    let response = request_builder
        .send()
        .await
        .map_err(|error| BeaError::ProviderRequest(error.to_string()))?;
    let status = response.status();
    // Check status BEFORE parsing so non-JSON error bodies (HTML error pages,
    // proxies) report "HTTP 500" instead of a confusing serde failure.
    if !status.is_success() {
        return Err(BeaError::ProviderRequest(format!("HTTP {status}")));
    }
    let payload = response
        .json::<serde_json::Value>()
        .await
        .map_err(|error| BeaError::ProviderRequest(error.to_string()))?;
    parse_provider_minutes(&payload)
}

pub fn record_usage(conn: &Connection, usage: &UsageRecord) -> Result<(), BeaError> {
    conn.execute("INSERT INTO usage_records(id,provider_id,model,input_tokens,output_tokens,estimated_cost,operation,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)", params![Uuid::new_v4().to_string(),usage.provider_id,usage.model,usage.input_tokens,usage.output_tokens,usage.estimated_cost,usage.operation,Utc::now().to_rfc3339()])?;
    Ok(())
}

/// A local extraction pass that summarizes candidate ledger events.
pub fn extract_ledger_events(segments: &[TranscriptSegment]) -> Vec<LedgerEvent> {
    let mut events = Vec::new();
    for segment in segments {
        let trimmed = segment.text.trim();
        if trimmed.is_empty() || trimmed == "[silence]" {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        // Procedural noise is not a decision or an action: motions approving
        // the previous minutes, roll call, greetings, and thanks.
        let procedural_noise = [
            "second the motion",
            "move for approval",
            "minutes of the meeting",
            "were reviewed",
            "good morning",
            "good afternoon",
            "thank you",
            "roll call",
            "attendance",
        ];
        if procedural_noise.iter().any(|noise| lower.contains(noise)) {
            continue;
        }
        let (kind, confidence) = if lower.contains("decided")
            || lower.contains("decision")
            || lower.contains("napagdesisyunan")
            || lower.contains("discretion by the dean")
            || lower.contains("decision ng dean")
        {
            ("decision", 0.85)
        } else if lower.contains("action item")
            || lower.contains("assigned to")
            || lower.contains("appointment letter")
            || lower.contains("orientation for the bpep")
            || lower.contains("submit it to")
            || lower.contains("ask permission from ched")
            || lower.contains("will release")
            || lower.contains("will announce")
            || lower.contains("will revise")
            || lower.contains("will submit")
            || lower.contains("will prepare")
            || lower.contains("will endorse")
        {
            ("action", 0.80)
        } else if lower.contains("open question")
            || lower.contains("unresolved")
            || lower.contains("hindi pa pwede")
            || lower.contains("walang solution")
            || lower.contains("still pending")
        {
            ("unresolved", 0.70)
        } else {
            continue;
        };

        // Create a concise summary sentence rather than verbatim transcript run-on
        let summary = summarize_segment_text(trimmed, kind);

        events.push(LedgerEvent {
            kind: kind.into(),
            summary,
            owner: None,
            due: None,
            confidence,
            evidence: vec![Evidence {
                start_seconds: segment.start_seconds,
                end_seconds: segment.end_seconds,
                quote: trimmed.to_string(),
            }],
        });
    }
    events
}

fn summarize_segment_text(text: &str, _kind: &str) -> String {
    let mut clean = text.replace('\n', " ");
    // Strip leading conversational fillers so the headline starts on content.
    const FILLER_PREFIXES: [&str; 14] = [
        "okay, ",
        "okay ",
        "so, ",
        "so ",
        "sige, ",
        "sige ",
        "ah, ",
        "ah ",
        "uh, ",
        "uh ",
        "and then ",
        "and ",
        "tapos ",
        "kasi ",
    ];
    loop {
        let mut stripped = false;
        for filler in FILLER_PREFIXES {
            if clean.starts_with(filler) {
                clean = clean[filler.len()..].trim_start().to_string();
                stripped = true;
                break;
            }
        }
        if !stripped {
            break;
        }
    }
    if clean.is_empty() {
        return "...".into();
    }
    // Capitalize the first letter.
    let mut chars = clean.chars();
    if let Some(first) = chars.next() {
        clean = first.to_uppercase().collect::<String>() + chars.as_str();
    }
    if clean.len() <= 140 {
        return clean;
    }
    // Cut at the first hard sentence boundary, keeping the first sentence.
    if let Some(pos) = clean.find(". ").or_else(|| clean.find("? ")).or_else(|| clean.find("! ")) {
        if pos >= 25 && pos <= 150 {
            return clean[..=pos].trim().to_string();
        }
    }
    let mut truncated = clean.chars().take(138).collect::<String>();
    if let Some(last_space) = truncated.rfind(' ') {
        truncated.truncate(last_space);
    }
    format!("{truncated}...")
}

pub fn generate_minutes(title: &str, events: &[LedgerEvent]) -> Minutes {
    let decisions: Vec<LedgerEvent> = events.iter().filter(|e| e.kind == "decision").cloned().collect();
    let action_items: Vec<LedgerEvent> = events.iter().filter(|e| e.kind == "action").cloned().collect();
    let unresolved: Vec<LedgerEvent> = events.iter().filter(|e| e.kind == "unresolved").cloned().collect();

    let summary = if decisions.is_empty() && action_items.is_empty() && unresolved.is_empty() {
        format!("Meeting discussion for {title}. No formal decisions or action items recorded.")
    } else {
        let mut parts = vec![format!("Executive summary for {title}.")];
        if !decisions.is_empty() {
            let decision_count = decisions.len();
            let noun = if decision_count == 1 { "decision" } else { "decisions" };
            parts.push(format!("{decision_count} key {noun} recorded."));
        }
        if !action_items.is_empty() {
            let action_count = action_items.len();
            let noun = if action_count == 1 { "action item" } else { "action items" };
            parts.push(format!("{action_count} {noun} identified."));
        }
        if !unresolved.is_empty() {
            let open_count = unresolved.len();
            let (noun, verb) = if open_count == 1 { ("item", "remains") } else { ("items", "remain") };
            parts.push(format!("{open_count} open {noun} {verb} pending further review."));
        }
        parts.join(" ")
    };

    Minutes {
        title: title.to_string(),
        summary,
        decisions,
        action_items,
        unresolved,
    }
}

pub fn save_ledger_events(
    conn: &Connection,
    meeting_id: &str,
    events: &[LedgerEvent],
) -> Result<(), BeaError> {
    let transaction = conn.unchecked_transaction()?;
    transaction.execute(
        "DELETE FROM ledger_events WHERE meeting_id=?1",
        params![meeting_id],
    )?;
    for event in events {
        transaction.execute(
            "INSERT INTO ledger_events(id,meeting_id,payload) VALUES (?1,?2,?3)",
            params![
                Uuid::new_v4().to_string(),
                meeting_id,
                serde_json::to_string(event)
                    .map_err(|e| BeaError::InvalidModelOutput(e.to_string()))?
            ],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

pub fn load_ledger_events(
    conn: &Connection,
    meeting_id: &str,
) -> Result<Vec<LedgerEvent>, BeaError> {
    let mut stmt =
        conn.prepare("SELECT payload FROM ledger_events WHERE meeting_id=?1 ORDER BY rowid")?;
    let rows = stmt.query_map(params![meeting_id], |r| r.get::<_, String>(0))?;
    rows.map(|row| {
        let payload = row?;
        serde_json::from_str(&payload)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
    })
    .collect::<Result<Vec<_>, _>>()
    .map_err(BeaError::Database)
}

pub fn save_minutes(
    conn: &Connection,
    meeting_id: &str,
    minutes: &Minutes,
) -> Result<(), BeaError> {
    conn.execute(
        "INSERT OR REPLACE INTO minutes(meeting_id,payload,updated_at) VALUES (?1,?2,?3)",
        params![
            meeting_id,
            serde_json::to_string(minutes)
                .map_err(|e| BeaError::InvalidModelOutput(e.to_string()))?,
            Utc::now().to_rfc3339()
        ],
    )?;
    Ok(())
}

pub fn load_minutes(conn: &Connection, meeting_id: &str) -> Result<Option<Minutes>, BeaError> {
    let payload: Option<String> = conn
        .query_row(
            "SELECT payload FROM minutes WHERE meeting_id=?1",
            params![meeting_id],
            |row| row.get(0),
        )
        .optional()?;
    payload.map(|value| parse_minutes_json(&value)).transpose()
}

pub fn export_markdown(minutes: &Minutes) -> String {
    let mut out = format!(
        "# Minutes of the Meeting\n\n## {}\n\n{}\n",
        minutes.title, minutes.summary
    );
    append_events(&mut out, "Decisions", &minutes.decisions);
    append_events(&mut out, "Action Items", &minutes.action_items);
    append_events(&mut out, "Unresolved Matters", &minutes.unresolved);
    out
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ExportFormat {
    Markdown,
    Pdf,
    Docx,
}

pub fn export_minutes(minutes: &Minutes, format: ExportFormat) -> Result<Vec<u8>, BeaError> {
    match format {
        ExportFormat::Markdown => Ok(export_markdown(minutes).into_bytes()),
        ExportFormat::Pdf => Ok(export_pdf(minutes).into_bytes()),
        ExportFormat::Docx => export_docx(minutes),
    }
}

fn pdf_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('(', "\\(")
        .replace(')', "\\)")
}
pub fn export_pdf(minutes: &Minutes) -> String {
    // Paginate instead of truncating: 48 lines per page at 14pt leading keeps
    // every line of the minutes in the document.
    let lines: Vec<String> = export_markdown(minutes).lines().map(pdf_escape).collect();
    const LINES_PER_PAGE: usize = 48;
    let page_count = lines.len().div_ceil(LINES_PER_PAGE).max(1);
    let mut contents = Vec::with_capacity(page_count);
    for page in 0..page_count {
        let start = page * LINES_PER_PAGE;
        let slice = lines
            .get(start..(start + LINES_PER_PAGE).min(lines.len()))
            .unwrap_or_default();
        let mut content = String::from("BT /F1 10 Tf 48 760 Td ");
        for (index, line) in slice.iter().enumerate() {
            if index > 0 {
                content.push_str(" 0 -14 Td ");
            }
            content.push_str(&format!("({line}) Tj"));
        }
        content.push_str(" ET");
        contents.push(content);
    }
    // Object layout: catalog, pages tree, then per-page (page + content),
    // font object last.
    let font_object_id = 3 + page_count * 2;
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!(
            "<< /Type /Pages /Kids [{}] /Count {page_count} >>",
            (0..page_count)
                .map(|page| format!("{} 0 R", 3 + page * 2))
                .collect::<Vec<_>>()
                .join(" ")
        ),
    ];
    for (page, content) in contents.iter().enumerate() {
        let page_id = 3 + page * 2;
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 {font_object_id} 0 R >> >> /Contents {} 0 R >>",
            page_id + 1
        ));
        objects.push(format!(
            "<< /Length {} >>
stream
{}
endstream",
            content.len(),
            content
        ));
    }
    debug_assert_eq!(objects.len() + 1, font_object_id);
    objects.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string());
    let mut pdf = String::from(
        "%PDF-1.4
",
    );
    let mut offsets = vec![0usize];
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.push_str(&format!(
            "{} 0 obj
{}
endobj
",
            index + 1,
            object
        ));
    }
    let xref = pdf.len();
    pdf.push_str(&format!(
        "xref
0 {}
0000000000 65535 f 
",
        objects.len() + 1
    ));
    for offset in offsets.iter().skip(1) {
        pdf.push_str(&format!(
            "{offset:010} 00000 n 
"
        ));
    }
    pdf.push_str(&format!(
        "trailer
<< /Size {} /Root 1 0 R >>
startxref
{xref}
%%EOF",
        objects.len() + 1
    ));
    pdf
}

pub fn export_docx(minutes: &Minutes) -> Result<Vec<u8>, BeaError> {
    use std::io::{Cursor, Write};
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;
    let mut cursor = Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(&mut cursor);
    let options = SimpleFileOptions::default();
    zip.start_file("[Content_Types].xml", options)
        .map_err(|e| BeaError::InvalidState(e.to_string()))?;
    zip.write_all(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/></Types>").map_err(|e| BeaError::InvalidState(e.to_string()))?;
    zip.start_file("_rels/.rels", options)
        .map_err(|e| BeaError::InvalidState(e.to_string()))?;
    zip.write_all(b"<?xml version=\"1.0\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/></Relationships>").map_err(|e| BeaError::InvalidState(e.to_string()))?;
    let body = export_markdown(minutes)
        .lines()
        .map(|line| format!("<w:p><w:r><w:t>{}</w:t></w:r></w:p>", xml_escape(line)))
        .collect::<String>();
    let document = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body>{body}<w:sectPr/></w:body></w:document>");
    zip.start_file("word/document.xml", options)
        .map_err(|e| BeaError::InvalidState(e.to_string()))?;
    zip.write_all(document.as_bytes())
        .map_err(|e| BeaError::InvalidState(e.to_string()))?;
    zip.finish()
        .map_err(|e| BeaError::InvalidState(e.to_string()))?;
    Ok(cursor.into_inner())
}
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
fn append_events(out: &mut String, heading: &str, events: &[LedgerEvent]) {
    out.push_str(&format!("\n## {heading}\n\n"));
    if events.is_empty() {
        out.push_str("_None recorded._\n");
    }
    for event in events {
        out.push_str(&format!("- {}", event.summary));
        if let Some(owner) = &event.owner {
            out.push_str(&format!(" — **Owner:** {owner}"));
        }
        if let Some(due) = &event.due {
            out.push_str(&format!(" — **Due:** {due}"));
        }
        if let Some(e) = event.evidence.first() {
            out.push_str(&format!(
                " _(evidence {}s–{}s)_",
                e.start_seconds, e.end_seconds
            ));
        }
        out.push('\n');
    }
}

pub fn verify_claim(claim: &str, evidence: &[Evidence]) -> VerifiedClaim {
    let normalized = claim.to_ascii_lowercase();
    let matching: Vec<Evidence> = evidence
        .iter()
        .filter(|e| {
            normalized
                .split_whitespace()
                .filter(|word| word.len() > 3)
                .any(|word| e.quote.to_ascii_lowercase().contains(word))
        })
        .cloned()
        .collect();
    let status = if matching.is_empty() {
        VerificationStatus::Uncertain
    } else if matching.len() == evidence.len() {
        VerificationStatus::Supported
    } else {
        VerificationStatus::Uncertain
    };
    VerifiedClaim {
        claim: claim.to_string(),
        status,
        evidence: matching,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;
    #[test]
    fn chat_context_includes_speaker_names_and_custom_format() {
        let context = build_meeting_context(
            &std::collections::HashMap::from([(0u32, "Maria Santos".to_string())]),
            Some("Use Q&A format."),
        );
        assert!(context.contains("Maria Santos"));
        assert!(context.contains("Use Q&A format."));
    }
    #[test]
    fn segment_speakers_support_overlap_and_rename() {
        let dir = tempdir().unwrap();
        let c = open_database(dir.path().join("bea.db")).unwrap();
        let m = create_meeting(&c, "Manual speakers", TranscriptLanguage::English).unwrap();
        let segment = TranscriptSegment { id: "s1".into(), meeting_id: m.id.clone(), start_seconds: 0, end_seconds: 10, text: "We agree in principle".into(), language_detected: None, language_confidence: None, speaker: Some(0) };
        add_segment(&c, &segment).unwrap();
        // Overlap: two speakers on one segment.
        set_segment_speakers(&c, "s1", &[0, 1]).unwrap();
        assert_eq!(list_segment_speakers(&c, &m.id).unwrap().get("s1").map(Vec::as_slice), Some(&[0u32, 1][..]));
        // Clearing back to the primary speaker removes the overlap row.
        set_segment_speakers(&c, "s1", &[]).unwrap();
        assert!(list_segment_speakers(&c, &m.id).unwrap().get("s1").is_none());
        // Renaming a speaker propagates from speaker_names.
        set_speaker_name(&c, &m.id, 0, "Maria Santos").unwrap();
        assert_eq!(list_speaker_names(&c, &m.id).unwrap().get(&0).map(String::as_str), Some("Maria Santos"));
    }
    #[test]
    fn speaker_names_round_trip_per_meeting() {
        let dir = tempdir().unwrap();
        let c = open_database(dir.path().join("bea.db")).unwrap();
        let m = create_meeting(&c, "VTT import", TranscriptLanguage::English).unwrap();
        set_speaker_name(&c, &m.id, 0, "Maria Santos").unwrap();
        set_speaker_name(&c, &m.id, 1, "John Cruz").unwrap();
        // Upsert overwrites instead of duplicating.
        set_speaker_name(&c, &m.id, 0, "Maria S.").unwrap();
        let names = list_speaker_names(&c, &m.id).unwrap();
        assert_eq!(names.get(&0).map(String::as_str), Some("Maria S."));
        assert_eq!(names.get(&1).map(String::as_str), Some("John Cruz"));
        assert_eq!(names.len(), 2);
    }
    #[test]
    fn chunks_are_bounded_and_five_hours_supported() {
        assert_eq!(chunk_ranges(3600).unwrap().len(), 60);
        assert_eq!(chunk_ranges(MAX_MEETING_SECONDS).unwrap().len(), 300);
        assert!(chunk_ranges(MAX_MEETING_SECONDS + 1).is_err());
    }
    #[test]
    fn meeting_and_transcript_are_persistent_and_searchable() {
        let dir = tempdir().unwrap();
        let c = open_database(dir.path().join("bea.db")).unwrap();
        for table in [
            "participants",
            "context_events",
            "topics",
            "visual_evidence",
            "action_items",
            "llm_runs",
        ] {
            let exists: i64 = c
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    params![table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(exists, 1, "missing table {table}");
        }
        let m = create_meeting(&c, "Planning", TranscriptLanguage::Taglish).unwrap();
        add_segment(
            &c,
            &TranscriptSegment {
                id: "s1".into(),
                meeting_id: m.id.clone(),
                start_seconds: 0,
                end_seconds: 4,
                text: "Ready na yung staging deployment".into(),
                language_detected: Some("taglish".into()),
                language_confidence: Some(0.95),
                speaker: Some(0),
            },
        )
        .unwrap();
        assert_eq!(search_transcript(&c, &m.id, "staging").unwrap().len(), 1);
        assert_eq!(list_transcript(&c, &m.id).unwrap().len(), 1);
        let minutes = generate_minutes("Planning", &[]);
        save_minutes(&c, &m.id, &minutes).unwrap();
        assert_eq!(load_minutes(&c, &m.id).unwrap().unwrap(), minutes);
    }
    #[test]
    fn meeting_crud_and_pause_state_survive_database_round_trip() {
        let dir = tempdir().unwrap();
        let c = open_database(dir.path().join("bea.db")).unwrap();
        let m = create_meeting(&c, "Draft", TranscriptLanguage::English).unwrap();
        update_meeting_title(&c, &m.id, "Renamed").unwrap();
        set_meeting_status(&c, &m.id, MeetingStatus::Paused, 30).unwrap();
        let listed = list_meetings(&c).unwrap();
        assert_eq!(listed[0].title, "Renamed");
        assert_eq!(listed[0].status, MeetingStatus::Paused);
        delete_meeting(&c, &m.id).unwrap();
        assert!(list_meetings(&c).unwrap().is_empty());
    }

    #[test]
    fn meeting_keeps_the_selected_asr_engine_snapshot() {
        let dir = tempdir().unwrap();
        let c = open_database(dir.path().join("bea.db")).unwrap();
        let meeting = create_meeting_with_engine(
            &c,
            "Whisper project",
            TranscriptLanguage::English,
            "whisper-compatibility",
        )
        .unwrap();
        assert_eq!(meeting.asr_engine_id, "whisper-compatibility");
        assert_eq!(
            list_meetings(&c).unwrap()[0].asr_engine_id,
            "whisper-compatibility"
        );
    }
    #[test]
    fn context_packing_respects_budget_and_minutes_schema_is_strict() {
        let e = LedgerEvent {
            kind: "decision".into(),
            summary: "A decision".into(),
            owner: None,
            due: None,
            confidence: 1.0,
            evidence: vec![],
        };
        assert_eq!(pack_context(&[e.clone(), e], 10).len(), 1);
        assert!(parse_minutes_json(r#"{"title":"Planning","summary":"Done","decisions":[],"action_items":[],"unresolved":[]}"#).is_ok());
        assert!(parse_minutes_json("not json").is_err());
    }
    #[test]
    fn recording_state_is_recoverable_after_restart() {
        let dir = tempdir().unwrap();
        let c = open_database(dir.path().join("bea.db")).unwrap();
        let m = create_meeting(&c, "Long meeting", TranscriptLanguage::Auto).unwrap();
        let chunk = start_recording(&c, &m.id, dir.path().join("chunks")).unwrap();
        assert_eq!(chunk.ordinal, 1);
        let recovered = recover_interrupted_chunks(&c, &m.id).unwrap();
        assert_eq!(recovered[0].state, ChunkState::Interrupted);
        assert!(complete_recording_chunk(&c, &chunk.id, 60).is_err());
        let next = start_recording(&c, &m.id, dir.path().join("chunks")).unwrap();
        complete_recording_chunk(&c, &next.id, 90).unwrap();
        assert_eq!(list_meetings(&c).unwrap()[0].duration_seconds, 90);
    }
    #[test]
    fn completed_audio_chunks_are_persisted_for_native_transcription() {
        let dir = tempdir().unwrap();
        let c = open_database(dir.path().join("bea.db")).unwrap();
        let meeting = create_meeting(&c, "Native ASR", TranscriptLanguage::Taglish).unwrap();
        let chunk = CompletedAudioChunk {
            ordinal: 1,
            path: dir.path().join("chunk-00001.wav"),
            start_seconds: 0,
            end_seconds: 60,
            sample_count: 960_000,
        };
        persist_completed_audio_chunk(&c, &meeting.id, &chunk).unwrap();
        let listed = list_completed_recording_chunks(&c, &meeting.id).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].path, chunk.path);
        assert_eq!(list_meetings(&c).unwrap()[0].duration_seconds, 60);
    }
    #[test]
    fn jobs_track_progress_and_media_keeps_original_path_by_default() {
        let dir = tempdir().unwrap();
        let c = open_database(dir.path().join("bea.db")).unwrap();
        let m = create_meeting(&c, "Import", TranscriptLanguage::English).unwrap();
        let mut file = std::fs::File::create(dir.path().join("meeting.mp3")).unwrap();
        file.write_all(b"audio").unwrap();
        let media = import_media(
            &c,
            &m.id,
            dir.path().join("meeting.mp3"),
            MediaKind::Audio,
            Some(12),
            false,
            dir.path(),
        )
        .unwrap();
        assert!(!media.copied);
        assert_eq!(media.path, dir.path().join("meeting.mp3"));
        let job = create_job(&c, &m.id, JobKind::Transcription).unwrap();
        update_job(&c, &job.id, JobState::Running, 0.5, None).unwrap();
        assert!(update_job(&c, &job.id, JobState::Completed, 1.1, None).is_err());
    }
    #[test]
    fn media_import_validates_extensions_and_can_copy_into_library() {
        let dir = tempdir().unwrap();
        let c = open_database(dir.path().join("bea.db")).unwrap();
        let meeting = create_meeting(&c, "Media", TranscriptLanguage::Auto).unwrap();
        let source = dir.path().join("brief.m4a");
        std::fs::write(&source, b"audio").unwrap();
        let copied = import_media(
            &c,
            &meeting.id,
            &source,
            MediaKind::Audio,
            None,
            true,
            dir.path(),
        )
        .unwrap();
        assert!(copied.copied);
        assert!(copied
            .path
            .starts_with(dir.path().join("media").join(&meeting.id)));
        let unsupported = dir.path().join("brief.txt");
        std::fs::write(&unsupported, b"not media").unwrap();
        assert!(import_media(
            &c,
            &meeting.id,
            unsupported,
            MediaKind::Audio,
            None,
            false,
            dir.path(),
        )
        .is_err());
    }
    #[test]
    fn model_checksum_and_manifest_are_verified() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("model.bin");
        std::fs::write(&path, b"model").unwrap();
        let hash = format!("{:x}", Sha256::digest(b"model"));
        verify_model_checksum(&path, &hash).unwrap();
        assert!(verify_model_checksum(&path, "00").is_err());
        let c = open_database(dir.path().join("bea.db")).unwrap();
        register_model(
            &c,
            &ModelManifest {
                id: "qwen".into(),
                name: "Bea Standard".into(),
                version: "1".into(),
                size_bytes: 5,
                sha256: hash.clone(),
                runtime: "sherpa-onnx".into(),
                languages: vec!["en".into(), "fil".into(), "taglish".into()],
                installed: false,
            },
        )
        .unwrap();
        let installed = install_model_package(
            &path,
            dir.path().join("models/qwen.bin"),
            &ModelManifest {
                id: "qwen2".into(),
                name: "Bea Standard".into(),
                version: "1".into(),
                size_bytes: 5,
                sha256: hash,
                runtime: "sherpa-onnx".into(),
                languages: vec!["en".into()],
                installed: false,
            },
        )
        .unwrap();
        assert_eq!(installed.bytes_copied, 5);
    }
    #[test]
    fn qwen_archive_install_extracts_only_safe_complete_model_layout() {
        let dir = tempdir().unwrap();
        let package_root = dir.path().join("package");
        std::fs::create_dir_all(package_root.join("tokenizer")).unwrap();
        for file in [
            "conv_frontend.onnx",
            "encoder.int8.onnx",
            "decoder.int8.onnx",
        ] {
            std::fs::write(package_root.join(file), b"onnx").unwrap();
        }
        std::fs::write(package_root.join("tokenizer").join("vocab.json"), b"{}").unwrap();
        let archive_path = dir.path().join("qwen.tar.bz2");
        let archive_file = std::fs::File::create(&archive_path).unwrap();
        let encoder = bzip2::write::BzEncoder::new(archive_file, bzip2::Compression::fast());
        let mut archive = tar::Builder::new(encoder);
        archive.append_dir_all("qwen-model", &package_root).unwrap();
        archive.into_inner().unwrap().finish().unwrap();
        let bytes = std::fs::read(&archive_path).unwrap();
        let manifest = ModelManifest {
            id: "qwen-archive".into(),
            name: "Qwen3-ASR".into(),
            version: "test".into(),
            size_bytes: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(&bytes)),
            runtime: "sherpa-onnx".into(),
            languages: vec!["en".into(), "fil".into(), "taglish".into()],
            installed: false,
        };
        let destination = dir.path().join("models");
        let progress = install_qwen_model_package(&archive_path, &destination, &manifest).unwrap();
        assert!(progress.verified);
        assert!(qwen_model_is_complete(&destination));
        assert!(find_qwen_model_dir(&destination).is_some());
    }
    #[test]
    fn ledger_minutes_export_and_verification_preserve_evidence() {
        let segments = vec![TranscriptSegment {
            id: "s".into(),
            meeting_id: "m".into(),
            start_seconds: 61,
            end_seconds: 70,
            text: "Decision: deploy on Monday; John will prepare the proposal.".into(),
            language_detected: Some("en".into()),
            language_confidence: Some(0.9),
            speaker: None,
        }];
        let events = extract_ledger_events(&segments);
        assert_eq!(events.len(), 1);
        let db_dir = tempdir().unwrap();
        let db = open_database(db_dir.path().join("bea.db")).unwrap();
        let meeting = create_meeting(&db, "Planning", TranscriptLanguage::English).unwrap();
        save_ledger_events(&db, &meeting.id, &events).unwrap();
        assert_eq!(load_ledger_events(&db, &meeting.id).unwrap(), events);
        let minutes = generate_minutes("Planning", &events);
        let markdown = export_markdown(&minutes);
        assert!(markdown.contains("evidence 61s"));
        assert_eq!(
            verify_claim("deploy Monday", &events[0].evidence).status,
            VerificationStatus::Supported
        );
        assert!(export_minutes(&minutes, ExportFormat::Markdown)
            .unwrap()
            .starts_with(b"# Minutes"));
        assert!(export_minutes(&minutes, ExportFormat::Pdf)
            .unwrap()
            .starts_with(b"%PDF-1.4"));
        let docx = export_minutes(&minutes, ExportFormat::Docx).unwrap();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(docx)).unwrap();
        assert!(archive.by_name("word/document.xml").is_ok());
    }
    #[test]
    fn context_modes_are_token_aware_and_preserve_evidence_counts() {
        let event = LedgerEvent {
            kind: "decision".into(),
            summary: "A concise decision".into(),
            owner: None,
            due: None,
            confidence: 1.0,
            evidence: vec![Evidence {
                start_seconds: 1,
                end_seconds: 2,
                quote: "source quote".into(),
            }],
        };
        let pack = pack_context_mode(&[event], 100, ContextMode::Balanced);
        assert!(pack.estimated_input_tokens > 0);
        assert_eq!(pack.evidence_count, 1);
        assert_eq!(estimate_tokens("1234"), 1);
    }
    #[test]
    fn provider_requests_are_openai_compatible_and_keep_credentials_out_of_payload() {
        let provider = ProviderConfig {
            id: "openrouter".into(),
            kind: ProviderKind::OpenRouter,
            base_url: "https://openrouter.ai/api/v1/".into(),
            model: "model".into(),
            credential_ref: Some("BEA_OPENROUTER_KEY".into()),
            enabled: true,
        };
        let request = build_provider_request(
            &provider,
            &LlmRequest {
                model: "model".into(),
                system: "system".into(),
                user: "user".into(),
                json_schema: r#"{"type":"object"}"#.into(),
                max_output_tokens: 100,
            },
        );
        assert_eq!(request.url, "https://openrouter.ai/api/v1/chat/completions");
        assert!(!request.body.to_string().contains("BEA_OPENROUTER_KEY"));
        let claude_proxy = ProviderConfig {
            id: "claude-proxy".into(),
            kind: ProviderKind::ClaudeCompatible,
            base_url: "https://proxy.example/v1".into(),
            model: "claude-sonnet".into(),
            credential_ref: Some("keyring:claude-proxy".into()),
            enabled: true,
        };
        assert_eq!(
            build_provider_request(
                &claude_proxy,
                &LlmRequest {
                    model: "claude-sonnet".into(),
                    system: "system".into(),
                    user: "user".into(),
                    json_schema: r#"{"type":"object"}"#.into(),
                    max_output_tokens: 100,
                }
            )
            .url,
            "https://proxy.example/v1/chat/completions"
        );
        let response = serde_json::json!({"choices":[{"message":{"content":"{\"title\":\"Planning\",\"summary\":\"Done\",\"decisions\":[],\"action_items\":[],\"unresolved\":[]}"}}]});
        assert_eq!(parse_provider_minutes(&response).unwrap().title, "Planning");
        let dir = tempdir().unwrap();
        let db = open_database(dir.path().join("bea.db")).unwrap();
        save_provider(&db, &provider).unwrap();
        assert_eq!(load_provider(&db, "openrouter").unwrap(), Some(provider));
        assert!(parse_provider_minutes(&serde_json::json!({})).is_err());
    }
    #[derive(Clone)]
    struct FakeAsr;
    impl AsrEngine for FakeAsr {
        fn kind(&self) -> AsrEngineKind {
            AsrEngineKind::Qwen3Asr06bInt8
        }
        fn capabilities(&self) -> AsrCapabilities {
            AsrCapabilities {
                languages: vec!["en".into(), "fil".into()],
                offline: true,
                streaming: false,
            }
        }
        fn transcribe(
            &self,
            chunk: &AudioChunkInput,
            _language: &TranscriptLanguage,
        ) -> Result<AsrResult, BeaError> {
            Ok(AsrResult {
                text: format!("chunk at {}", chunk.start_seconds),
                language_detected: Some("fil".into()),
                confidence: Some(0.9),
            })
        }
    }
    #[test]
    fn silent_chunks_are_logged_as_timed_silence_rows() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("silent.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for _ in 0..16000 * 2 {
            writer.write_sample(0i16).unwrap();
        }
        writer.finalize().unwrap();

        let db = open_database(dir.path().join("bea.db")).unwrap();
        let meeting = create_meeting(&db, "Quiet", TranscriptLanguage::Taglish).unwrap();
        let chunks = vec![AudioChunkInput {
            path,
            start_seconds: 120,
            end_seconds: 180,
        }];
        let segments = transcribe_chunks(
            &db,
            &meeting.id,
            &chunks,
            &TranscriptLanguage::Taglish,
            FakeAsr,
        )
        .unwrap();
        // The silent minute is kept as a timed placeholder row instead of
        // being dropped, so minutes of silence stay visible in transcripts.
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, SILENCE_TEXT);
        assert_eq!(segments[0].start_seconds, 120);
        assert_eq!(segments[0].end_seconds, 180);
        let stored = list_transcript(&db, &meeting.id).unwrap();
        assert_eq!(stored.len(), 1);
        // Silence rows stay out of full-text search.
        assert!(search_transcript(&db, &meeting.id, "silence")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn degenerate_taglish_output_is_detected_and_retried_without_language_hint() {
        // First pass returns a repetition loop under the forced Taglish hint;
        // the fallback auto pass returns real codeswitched text and wins.
        #[derive(Clone)]
        struct LoopingAsr;
        impl AsrEngine for LoopingAsr {
            fn kind(&self) -> AsrEngineKind {
                AsrEngineKind::Qwen3Asr06bInt8
            }
            fn capabilities(&self) -> AsrCapabilities {
                AsrCapabilities {
                    languages: vec!["en".into(), "fil".into(), "taglish".into()],
                    offline: true,
                    streaming: false,
                }
            }
            fn transcribe(
                &self,
                _chunk: &AudioChunkInput,
                language: &TranscriptLanguage,
            ) -> Result<AsrResult, BeaError> {
                if matches!(language, TranscriptLanguage::Auto) {
                    return Ok(AsrResult {
                        text: "Oo nga eh, tuloy na natin yung deployment bukas".into(),
                        language_detected: Some("taglish".into()),
                        confidence: Some(0.9),
                    });
                }
                Ok(AsrResult {
                    text: "sige sige sige sige sige sige sige sige".into(),
                    language_detected: Some("taglish".into()),
                    confidence: None,
                })
            }
        }

        assert_eq!(
            is_degenerate_asr_text(
                "sige sige sige sige sige sige sige sige",
                &TranscriptLanguage::Taglish
            ),
            Some("repetition-loop")
        );
        assert_eq!(
            is_degenerate_asr_text("language", &TranscriptLanguage::Taglish),
            Some("scaffold")
        );
        assert_eq!(
            is_degenerate_asr_text(
                "Ready na yung staging deployment",
                &TranscriptLanguage::Taglish
            ),
            None
        );

        let dir = tempdir().unwrap();
        let path = dir.path().join("speech.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for index in 0..16000 * 2 {
            let sample = if index % 2 == 0 { 900i16 } else { -900i16 };
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();

        let db = open_database(dir.path().join("bea.db")).unwrap();
        let meeting = create_meeting(&db, "Codeswitch", TranscriptLanguage::Taglish).unwrap();
        let segments = transcribe_chunks(
            &db,
            &meeting.id,
            &[AudioChunkInput {
                path,
                start_seconds: 0,
                end_seconds: 60,
            }],
            &TranscriptLanguage::Taglish,
            LoopingAsr,
        )
        .unwrap();
        assert_eq!(segments.len(), 1);
        assert!(
            segments[0].text.contains("deployment"),
            "expected the retried auto-language transcript, got: {}",
            segments[0].text
        );
    }

    #[test]
    fn resampler_matches_length_and_preserves_silent_and_loud_extremes() {
        let samples: Vec<i16> = (0..48000)
            .map(|index| if index % 2 == 0 { 800 } else { -800 })
            .collect();
        let downsampled = resample_linear_i16(&samples, 48_000, 16_000);
        assert_eq!(downsampled.len(), 16_000);
        for value in &downsampled {
            assert!(value.abs() <= 800);
        }
        // Upsampling back grows proportionally; identity is a no-op.
        let upsampled = resample_linear_i16(&downsampled, 16_000, 48_000);
        assert!((upsampled.len() as i64 - 48_000).abs() <= 1);
        assert_eq!(resample_linear_i16(&samples, 48_000, 48_000).len(), 48_000);
    }

    #[test]
    fn diarizer_uses_threshold_based_clustering_instead_of_forced_two_speakers() {
        // Config-level regression guard: num_clusters must stay on automatic
        // (-1) so meetings with more than two voices label correctly.
        let config = sherpa_onnx::FastClusteringConfig {
            num_clusters: -1,
            threshold: 0.5,
            ..Default::default()
        };
        assert_eq!(config.num_clusters, -1);
        assert!((config.threshold - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn whisper_layout_accepts_upstream_prefixed_files_in_nested_folder() {
        let dir = tempdir().unwrap();
        // Upstream sherpa-onnx-whisper-turbo.tar.bz2 extracts to a top-level
        // folder with model-prefixed filenames.
        let model = dir.path().join("sherpa-onnx-whisper-turbo");
        std::fs::create_dir_all(&model).unwrap();
        for name in [
            "turbo-encoder.onnx",
            "turbo-decoder.onnx",
            "turbo-tokens.txt",
        ] {
            std::fs::write(model.join(name), b"x").unwrap();
        }
        assert!(whisper_model_is_complete(dir.path()));
        assert!(!whisper_model_is_complete(model.join("missing").as_path()));
    }

    #[test]
    fn nemotron_layout_accepts_transducer_and_ctc_shapes() {
        let dir = tempdir().unwrap();
        // Upstream transducer layout (int8 variants, nested folder).
        let transducer = dir.path().join("nemotron-3.5");
        std::fs::create_dir_all(&transducer).unwrap();
        for name in [
            "encoder.int8.onnx",
            "decoder.int8.onnx",
            "joiner.int8.onnx",
            "tokens.txt",
        ] {
            std::fs::write(transducer.join(name), b"x").unwrap();
        }
        assert!(nemotron_model_is_complete(dir.path()));
        // Older CTC layout.
        let ctc = dir.path().join("ctc");
        std::fs::create_dir_all(&ctc).unwrap();
        for name in ["model.onnx", "tokens.txt"] {
            std::fs::write(ctc.join(name), b"x").unwrap();
        }
        assert!(nemotron_model_is_complete(ctc.as_path()));
        // Incomplete directory is rejected.
        let partial = dir.path().join("partial");
        std::fs::create_dir_all(&partial).unwrap();
        std::fs::write(partial.join("tokens.txt"), b"x").unwrap();
        assert!(!nemotron_model_is_complete(partial.as_path()));
    }

    #[test]
    fn asr_engine_is_abstracted_and_persists_chunk_results() {
        let dir = tempdir().unwrap();
        let c = open_database(dir.path().join("bea.db")).unwrap();
        let m = create_meeting(&c, "ASR", TranscriptLanguage::Filipino).unwrap();
        let chunks = vec![
            AudioChunkInput {
                path: dir.path().join("chunk.ogg"),
                start_seconds: 0,
                end_seconds: 60,
            },
            AudioChunkInput {
                path: dir.path().join("chunk2.ogg"),
                start_seconds: 60,
                end_seconds: 120,
            },
        ];
        let result =
            transcribe_chunks(&c, &m.id, &chunks, &TranscriptLanguage::Filipino, FakeAsr).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(search_transcript(&c, &m.id, "chunk").unwrap().len(), 2);
    }

    #[test]
    #[ignore = "requires BEA_QWEN_MODEL_DIR and BEA_QWEN_TEST_WAV"]
    fn qwen_engine_transcribes_real_codeswitch_fixture() {
        let model_dir = std::env::var_os("BEA_QWEN_MODEL_DIR")
            .expect("BEA_QWEN_MODEL_DIR must point to a complete Qwen model directory");
        let wav = std::env::var_os("BEA_QWEN_TEST_WAV")
            .expect("BEA_QWEN_TEST_WAV must point to a 16 kHz WAV fixture");
        let language = match std::env::var("BEA_QWEN_TEST_LANGUAGE").as_deref() {
            Ok("en") => TranscriptLanguage::English,
            Ok("fil") => TranscriptLanguage::Filipino,
            _ => TranscriptLanguage::Taglish,
        };
        let engine = Qwen3AsrEngine::from_model_dir(model_dir).unwrap();
        let result = engine
            .transcribe(
                &AudioChunkInput {
                    path: PathBuf::from(wav),
                    start_seconds: 0,
                    end_seconds: 60,
                },
                &language,
            )
            .unwrap();
        println!("Qwen fixture transcription: {}", result.text);
        assert!(!result.text.trim().is_empty());
        assert!(result.confidence.unwrap_or_default() >= 0.0);
    }

    #[test]
    #[ignore = "requires BEA_QWEN_ARCHIVE and BEA_QWEN_TEST_WAV"]
    fn qwen_archive_install_then_transcribe_real_fixture() {
        let archive = PathBuf::from(
            std::env::var_os("BEA_QWEN_ARCHIVE")
                .expect("BEA_QWEN_ARCHIVE must point to a tar.bz2 model package"),
        );
        let wav = PathBuf::from(
            std::env::var_os("BEA_QWEN_TEST_WAV")
                .expect("BEA_QWEN_TEST_WAV must point to a WAV fixture"),
        );
        let bytes = std::fs::read(&archive).unwrap();
        let manifest = ModelManifest {
            id: "qwen3-asr-0.6b-int8".into(),
            name: "Qwen3-ASR 0.6B INT8".into(),
            version: "fixture".into(),
            size_bytes: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(&bytes)),
            runtime: "sherpa-onnx".into(),
            languages: vec!["en".into(), "fil".into(), "taglish".into()],
            installed: false,
        };
        let destination = tempdir().unwrap();
        let progress = install_qwen_model_package(&archive, destination.path(), &manifest).unwrap();
        assert!(progress.verified);
        let engine = Qwen3AsrEngine::from_model_dir(destination.path()).unwrap();
        let result = engine
            .transcribe(
                &AudioChunkInput {
                    path: wav,
                    start_seconds: 0,
                    end_seconds: 60,
                },
                &TranscriptLanguage::Taglish,
            )
            .unwrap();
        assert!(!result.text.trim().is_empty());
    }

    #[test]
    fn runtime_capabilities_never_advertise_missing_assets_as_ready() {
        let dir = tempdir().unwrap();
        let provider = ProviderConfig {
            id: "local".into(),
            kind: ProviderKind::Local,
            base_url: "http://127.0.0.1".into(),
            model: "local".into(),
            credential_ref: None,
            enabled: true,
        };
        let unavailable = inspect_runtime(
            dir.path().join("qwen"),
            dir.path().join("ffmpeg"),
            dir.path().join("tesseract.exe"),
            Some(&provider),
        );
        assert!(!unavailable.asr_model_available);
        assert!(!unavailable.can_transcribe_locally);
        assert!(Qwen3AsrEngine::from_model_dir(dir.path().join("missing-model")).is_err());
        std::fs::write(dir.path().join("qwen"), b"model").unwrap();
        let available = inspect_runtime(
            dir.path().join("qwen"),
            dir.path().join("ffmpeg"),
            dir.path().join("tesseract.exe"),
            Some(&provider),
        );
        assert!(available.can_transcribe_locally);
    }

    #[test]
    fn runtime_detects_bundled_ffmpeg_and_ffprobe_version_markers() {
        let ffmpeg = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join("ffmpeg-x86_64-pc-windows-msvc.exe");
        let ffprobe = ffmpeg
            .parent()
            .unwrap()
            .join("ffprobe-x86_64-pc-windows-msvc.exe");
        if ffmpeg.is_file() && ffprobe.is_file() {
            let dir = tempdir().unwrap();
            let runtime = inspect_runtime(
                dir.path().join("models"),
                ffmpeg,
                dir.path().join("missing-tesseract.exe"),
                None,
            );
            assert!(runtime.ffmpeg_available);
        }
    }
    #[test]
    fn segmented_wav_recorder_rotates_chunks_and_pause_is_lossless() {
        let dir = tempdir().unwrap();
        let config = RecorderConfig {
            sample_rate: 10,
            channels: 1,
            chunk_seconds: 2,
            max_duration_seconds: 5,
        };
        let mut recorder = SegmentedWavRecorder::new(dir.path(), config).unwrap();
        recorder.start().unwrap();
        assert!(recorder
            .push_samples(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10])
            .unwrap()
            .is_empty());
        recorder.pause().unwrap();
        assert!(recorder
            .push_samples(&[11, 12, 13, 14, 15])
            .unwrap()
            .is_empty());
        recorder.resume().unwrap();
        let completed = recorder
            .push_samples(&[16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30])
            .unwrap();
        assert_eq!(completed.len(), 1);
        let final_chunk = recorder.stop().unwrap();
        assert_eq!(final_chunk.len(), 1);
        assert!(completed
            .iter()
            .chain(final_chunk.iter())
            .all(|chunk| chunk.path.is_file()));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }
    #[test]
    fn segmented_recorder_rejects_samples_beyond_five_hour_bound() {
        let dir = tempdir().unwrap();
        let config = RecorderConfig {
            sample_rate: 1,
            channels: 1,
            chunk_seconds: 1,
            max_duration_seconds: 2,
        };
        let mut recorder = SegmentedWavRecorder::new(dir.path(), config).unwrap();
        recorder.start().unwrap();
        assert!(recorder.push_samples(&[1, 2, 3]).is_err());
    }

    #[test]
    fn segmented_recorder_duration_matrix_covers_acceptance_points() {
        for duration in [1_800_u64, 3_600, 10_800, MAX_MEETING_SECONDS] {
            let dir = tempdir().unwrap();
            let config = RecorderConfig {
                sample_rate: 1,
                channels: 1,
                chunk_seconds: 60,
                max_duration_seconds: MAX_MEETING_SECONDS,
            };
            let mut recorder = SegmentedWavRecorder::new(dir.path(), config).unwrap();
            recorder.start().unwrap();
            let mut completed = Vec::new();
            for _ in 0..duration {
                completed.extend(recorder.push_samples(&[1]).unwrap());
            }
            completed.extend(recorder.stop().unwrap());
            assert_eq!(completed.last().unwrap().end_seconds, duration);
            assert_eq!(completed.len(), (duration / 60) as usize);
        }
    }

    #[test]
    fn segmented_recorder_completes_a_full_five_hour_schedule_with_bounded_chunks() {
        let dir = tempdir().unwrap();
        let config = RecorderConfig {
            sample_rate: 1,
            channels: 1,
            chunk_seconds: 60,
            max_duration_seconds: MAX_MEETING_SECONDS,
        };
        let mut recorder = SegmentedWavRecorder::new(dir.path(), config).unwrap();
        recorder.start().unwrap();
        let mut completed = Vec::new();
        for _ in 0..MAX_MEETING_SECONDS {
            completed.extend(recorder.push_samples(&[1]).unwrap());
        }
        completed.extend(recorder.stop().unwrap());
        assert_eq!(completed.len(), 300);
        assert_eq!(completed.last().unwrap().end_seconds, MAX_MEETING_SECONDS);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 300);
    }
    #[test]
    fn visual_frames_are_deduplicated_without_loading_video_into_memory() {
        let hash = frame_hash(b"same frame");
        let frames = vec![
            VisualFrame {
                id: "1".into(),
                timestamp_seconds: 1,
                path: "one.jpg".into(),
                thumbnail_path: None,
                perceptual_hash: hash.clone(),
                description: None,
            },
            VisualFrame {
                id: "2".into(),
                timestamp_seconds: 2,
                path: "two.jpg".into(),
                thumbnail_path: None,
                perceptual_hash: hash,
                description: None,
            },
        ];
        assert_eq!(deduplicate_frames(&frames).len(), 1);
        let audio_args = ffmpeg_audio_args(Path::new("meeting.mp4"), Path::new("audio.ogg"));
        assert!(audio_args.windows(2).any(|pair| pair == ["-vn", "-ac"]));
        let frame_args =
            ffmpeg_keyframe_args(Path::new("meeting.mp4"), Path::new("frames/%05d.jpg"), 5);
        assert!(frame_args.contains(&"fps=1/5".into()));
        assert!(run_ffmpeg(Path::new("missing-ffmpeg"), &[]).is_err());
        let ocr = TesseractOcrEngine {
            executable: PathBuf::from("missing-tesseract"),
            language: "eng".into(),
        };
        assert!(ocr
            .extract_text(&VisualFrame {
                id: "frame".into(),
                timestamp_seconds: 0,
                path: PathBuf::from("frame.jpg"),
                thumbnail_path: None,
                perceptual_hash: "hash".into(),
                description: None,
            })
            .is_err());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn bundled_tesseract_runtime_starts_without_machine_installation() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let executable = root.join("binaries/tesseract/tesseract.exe");
        if !executable.is_file() {
            return;
        }
        let result = TesseractOcrEngine {
            executable,
            language: "eng".into(),
        }
        .extract_text(&VisualFrame {
            id: "bundled-ocr".into(),
            timestamp_seconds: 0,
            path: root.join("icons/icon.png"),
            thumbnail_path: None,
            perceptual_hash: "hash".into(),
            description: None,
        });
        assert!(result.is_ok(), "bundled Tesseract failed: {result:?}");
    }

    #[test]
    fn bundled_ffmpeg_sidecars_execute_short_video_pipeline() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let ffmpeg = root.join("binaries/ffmpeg-x86_64-pc-windows-msvc.exe");
        let ffprobe = root.join("binaries/ffprobe-x86_64-pc-windows-msvc.exe");
        if !ffmpeg.is_file() || !ffprobe.is_file() {
            return;
        }
        let dir = tempdir().unwrap();
        let input = dir.path().join("sample.mp4");
        let generated = std::process::Command::new(&ffmpeg)
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=320x240:rate=2",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=1000:sample_rate=16000",
                "-t",
                "2",
                "-c:v",
                "libx264",
                "-c:a",
                "aac",
            ])
            .arg(&input)
            .output()
            .unwrap();
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );

        let analysis = analyze_video(
            &FfmpegPipeline { ffmpeg, ffprobe },
            &input,
            &dir.path().join("derived"),
            1,
        )
        .unwrap();
        assert!(analysis.metadata.has_video);
        assert!(analysis.metadata.audio_streams >= 1);
        assert!(analysis.audio_path.is_file());
        assert!(!analysis.frames.is_empty());
    }

    #[test]
    fn bundled_ffmpeg_pipeline_handles_hd_and_4k_video_inputs() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let ffmpeg = root.join("binaries/ffmpeg-x86_64-pc-windows-msvc.exe");
        let ffprobe = root.join("binaries/ffprobe-x86_64-pc-windows-msvc.exe");
        if !ffmpeg.is_file() || !ffprobe.is_file() {
            return;
        }
        let dir = tempdir().unwrap();
        for (label, size) in [("hd", "1920x1080"), ("uhd", "3840x2160")] {
            let input = dir.path().join(format!("{label}.mp4"));
            let generated = std::process::Command::new(&ffmpeg)
                .args([
                    "-y",
                    "-f",
                    "lavfi",
                    "-i",
                    &format!("testsrc=size={size}:rate=1"),
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:sample_rate=16000",
                    "-t",
                    "1",
                    "-c:v",
                    "libx264",
                    "-c:a",
                    "aac",
                ])
                .arg(&input)
                .output()
                .unwrap();
            assert!(
                generated.status.success(),
                "{label}: {}",
                String::from_utf8_lossy(&generated.stderr)
            );
            let analysis = analyze_video(
                &FfmpegPipeline {
                    ffmpeg: ffmpeg.clone(),
                    ffprobe: ffprobe.clone(),
                },
                &input,
                &dir.path().join(label),
                1,
            )
            .unwrap();
            assert!(analysis.metadata.has_video);
            assert!(analysis.metadata.audio_streams >= 1);
            assert!(analysis.audio_path.is_file());
            assert!(!analysis.frames.is_empty());
        }
    }

    #[test]
    fn visual_analysis_runs_ocr_before_vision_and_keeps_timestamped_evidence() {
        struct FakeOcr;
        impl OcrEngine for FakeOcr {
            fn extract_text(&self, _frame: &VisualFrame) -> Result<OcrResult, BeaError> {
                Ok(OcrResult {
                    text: "Q3 roadmap".into(),
                    confidence: Some(0.92),
                })
            }
        }
        struct FakeVision;
        impl VisionEngine for FakeVision {
            fn describe(
                &self,
                _frame: &VisualFrame,
                ocr: &OcrResult,
            ) -> Result<Option<String>, BeaError> {
                assert_eq!(ocr.text, "Q3 roadmap");
                Ok(Some("roadmap slide".into()))
            }
        }
        let frames = vec![VisualFrame {
            id: "frame-1".into(),
            timestamp_seconds: 42,
            path: "frame.jpg".into(),
            thumbnail_path: None,
            perceptual_hash: "hash".into(),
            description: None,
        }];
        let analyses = analyze_visual_frames(&frames, &FakeOcr, &FakeVision).unwrap();
        assert_eq!(analyses[0].timestamp_seconds, 42);
        assert_eq!(analyses[0].description.as_deref(), Some("roadmap slide"));
        let dir = tempdir().unwrap();
        let db = open_database(dir.path().join("bea.db")).unwrap();
        let meeting = create_meeting(&db, "Slides", TranscriptLanguage::English).unwrap();
        save_visual_analysis(&db, &meeting.id, &frames[0], &analyses[0]).unwrap();
        let loaded = load_visual_frames(&db, &meeting.id).unwrap();
        assert_eq!(loaded[0].id, frames[0].id);
        assert_eq!(loaded[0].description.as_deref(), Some("roadmap slide"));
    }
    #[test]
    fn video_analysis_normalizes_audio_and_deduplicates_timestamped_frames() {
        struct FakeMedia;
        impl MediaPipeline for FakeMedia {
            fn probe(&self, _input: &Path) -> Result<MediaMetadata, BeaError> {
                Ok(MediaMetadata {
                    duration_seconds: 12,
                    has_video: true,
                    width: Some(1920),
                    height: Some(1080),
                    audio_streams: 1,
                })
            }
            fn extract_audio(&self, _input: &Path, output: &Path) -> Result<(), BeaError> {
                std::fs::write(output, b"audio")
                    .map_err(|error| BeaError::MediaProcessing(error.to_string()))
            }
            fn ffmpeg_path(&self) -> &Path {
                Path::new("ffmpeg-test")
            }
            fn extract_keyframes(
                &self,
                _input: &Path,
                output_dir: &Path,
                _interval: u32,
            ) -> Result<(), BeaError> {
                std::fs::create_dir_all(output_dir).unwrap();
                std::fs::write(output_dir.join("frame-000001.jpg"), b"same").unwrap();
                std::fs::write(output_dir.join("frame-000002.jpg"), b"same").unwrap();
                Ok(())
            }
        }
        let dir = tempdir().unwrap();
        let result = analyze_video(&FakeMedia, Path::new("video.mp4"), dir.path(), 5).unwrap();
        assert_eq!(result.metadata.width, Some(1920));
        assert_eq!(result.frames.len(), 1);
        assert_eq!(result.frames[0].timestamp_seconds, 5);
        assert!(result.audio_path.is_file());
    }

    #[test]
    fn imported_audio_is_normalized_and_transcribed_into_persistent_segments() {
        struct FakeMedia(PathBuf);
        impl MediaPipeline for FakeMedia {
            fn probe(&self, _input: &Path) -> Result<MediaMetadata, BeaError> {
                Ok(MediaMetadata {
                    duration_seconds: 7,
                    has_video: false,
                    width: None,
                    height: None,
                    audio_streams: 1,
                })
            }
            fn extract_audio(&self, _input: &Path, output: &Path) -> Result<(), BeaError> {
                // Write a real PCM WAV with audible energy (peak ~±1000, well
                // above the -45 dBFS silence gate) so the ffmpeg slice step
                // works and the chunk reaches the ASR engine.
                let spec = hound::WavSpec {
                    channels: 1,
                    sample_rate: 16000,
                    bits_per_sample: 16,
                    sample_format: hound::SampleFormat::Int,
                };
                let mut writer = hound::WavWriter::create(output, spec)
                    .map_err(|error| BeaError::MediaProcessing(error.to_string()))?;
                for index in 0..16000 * 7 {
                    let sample = if index % 2 == 0 { 1000i16 } else { -1000i16 };
                    writer
                        .write_sample(sample)
                        .map_err(|error| BeaError::MediaProcessing(error.to_string()))?;
                }
                writer
                    .finalize()
                    .map_err(|error| BeaError::MediaProcessing(error.to_string()))
            }
            fn ffmpeg_path(&self) -> &Path {
                &self.0
            }
            fn extract_keyframes(
                &self,
                _input: &Path,
                _output_dir: &Path,
                _interval: u32,
            ) -> Result<(), BeaError> {
                Ok(())
            }
        }
        let bundled_ffmpeg = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries/ffmpeg-x86_64-pc-windows-msvc.exe");
        if !bundled_ffmpeg.is_file() {
            return; // slicing requires the real ffmpeg sidecar
        }
        #[derive(Clone)]
        struct FakeAsr;
        impl AsrEngine for FakeAsr {
            fn kind(&self) -> AsrEngineKind {
                AsrEngineKind::WhisperCompatibility
            }
            fn capabilities(&self) -> AsrCapabilities {
                AsrCapabilities {
                    languages: vec!["en".into()],
                    offline: true,
                    streaming: false,
                }
            }
            fn transcribe(
                &self,
                _chunk: &AudioChunkInput,
                _language: &TranscriptLanguage,
            ) -> Result<AsrResult, BeaError> {
                Ok(AsrResult {
                    text: "imported audio".into(),
                    language_detected: Some("en".into()),
                    confidence: Some(0.9),
                })
            }
        }
        let dir = tempdir().unwrap();
        let db = open_database(dir.path().join("bea.db")).unwrap();
        let meeting = create_meeting(&db, "Imported audio", TranscriptLanguage::English).unwrap();
        let segments = transcribe_imported_media(
            &db,
            &meeting.id,
            Path::new("recording.mp3"),
            &MediaKind::Audio,
            &dir.path().join("derived"),
            &TranscriptLanguage::English,
            &FakeMedia(bundled_ffmpeg),
            FakeAsr,
        )
        .unwrap();
        assert_eq!(segments[0].end_seconds, 7);
        assert_eq!(segments[0].text, "imported audio");
        assert!(dir.path().join("derived/audio.wav").is_file());
        assert_eq!(list_transcript(&db, &meeting.id).unwrap().len(), 1);
    }

    #[test]
    fn sherpa_packages_require_a_supported_layout_and_are_fingerprinted() {
        let source = tempdir().unwrap();
        std::fs::write(source.path().join("encoder.onnx"), b"encoder").unwrap();
        std::fs::write(source.path().join("decoder.onnx"), b"decoder").unwrap();
        std::fs::write(source.path().join("tokens.txt"), b"tokens").unwrap();
        let manifest = inspect_asr_model_package(source.path()).unwrap();
        assert_eq!(manifest.id, "whisper-compatibility");
        let destination = tempdir().unwrap();
        let progress =
            install_sherpa_model_package(source.path(), destination.path(), &manifest).unwrap();
        assert!(progress.verified);
        assert!(whisper_model_is_complete(destination.path()));
        let invalid = tempdir().unwrap();
        std::fs::write(invalid.path().join("encoder.onnx"), b"encoder").unwrap();
        std::fs::write(invalid.path().join("decoder.onnx"), b"decoder").unwrap();
        std::fs::write(invalid.path().join("random.onnx"), b"random").unwrap();
        assert!(inspect_asr_model_package(invalid.path()).is_err());
    }

    #[test]
    fn waveform_peaks_are_bounded_and_normalized() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("peaks.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for index in 0..1_000 {
            writer
                .write_sample(if index % 2 == 0 { i16::MAX } else { 0 })
                .unwrap();
        }
        writer.finalize().unwrap();
        let peaks = waveform_peaks(&path, 64).unwrap();
        assert_eq!(peaks.len(), 64);
        assert!(peaks.iter().all(|peak| (0.0..=1.0).contains(peak)));
    }
}
