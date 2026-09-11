// Smoke probe for Bea's transcription speed + resource optimizations and the
// GPU (DirectML) fail-safe path. Runs the real engines on real model files:
//
//   cargo run --release --example resource_probe -- <models_root> [whisper|qwen]
//
// Phases:
//   machine  — cores/threads/turbo gate, baseline memory
//   pipeline — prepare_imported_media_chunks on a ~150 s fixture (ffmpeg)
//   diarize  — small models load, produce turns, are dropped (snapshot proves it)
//   gpu      — DirectML session attempt on the CPU-only runtime (expect: graceful CPU fallback)
//   speed    — sequential vs turbo (2 independent engines) over the same chunks
//   trim     — process working set before/after trim_process_memory
use bea_core::{
    active_asr_device, asr_thread_count, clear_transcript, clear_transcription_progress,
    load_asr_engine, open_database, prepare_imported_media_chunks, process_memory_snapshot,
    read_wav_samples_resampled_capped, run_at_below_normal,
    transcribe_chunks_multi_engine_with_progress, transcribe_chunks_with_progress,
    trim_process_memory, turbo_supported, AsrDevice, AsrEngine, AudioChunkInput,
    ConfiguredAsrEngine, FfmpegPipeline, MediaKind, Qwen3AsrEngine, SpeakerDiarizer,
    TranscriptLanguage,
};
use std::path::{Path, PathBuf};
use std::time::Instant;

fn report(label: &str) {
    let snapshot = process_memory_snapshot();
    let helpers: u64 = snapshot.helper_processes.iter().map(|(_, mb)| *mb).sum();
    let detail = snapshot
        .helper_processes
        .iter()
        .map(|(name, mb)| format!("{name}={mb}MB"))
        .collect::<Vec<_>>()
        .join(", ");
    println!(
        "[mem] {label}: bea={}MB helpers={helpers}MB ({detail})",
        snapshot.bea_mb
    );
}

/// Concatenates one short fixture wav on loop into a ~150 s 16 kHz mono WAV
/// with the bundled ffmpeg — enough 28 s chunks for a turbo comparison.
fn make_long_fixture(ffmpeg: &Path, source: &Path, output: &Path) -> Result<(), String> {
    let args = vec![
        "-y".to_string(),
        "-stream_loop".to_string(),
        "-1".to_string(),
        "-i".to_string(),
        source.to_string_lossy().into_owned(),
        "-t".to_string(),
        "150".to_string(),
        "-vn".to_string(),
        "-ac".to_string(),
        "1".to_string(),
        "-ar".to_string(),
        "16000".to_string(),
        "-c:a".to_string(),
        "pcm_s16le".to_string(),
        output.to_string_lossy().into_owned(),
    ];
    bea_core::run_ffmpeg(ffmpeg, &args).map_err(|error| error.to_string())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let models_root = PathBuf::from(args.get(1).expect("usage: resource_probe <models_root> [whisper|qwen]"));
    let which = args.get(2).map(String::as_str).unwrap_or("whisper");
    let (engine_id, engine_dir, fixture): (String, PathBuf, PathBuf) = match which {
        "qwen" => (
            "qwen-standard".into(),
            models_root.join("qwen3-asr-0.6b-int8"),
            models_root
                .join("qwen3-asr-0.6b-int8")
                .join("sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25")
                .join("test_wavs")
                .join("fast1.wav"),
        ),
        _ => (
            "whisper-compatibility".into(),
            models_root.join("whisper-compatibility"),
            models_root
                .join("whisper-compatibility")
                .join("sherpa-onnx-whisper-turbo")
                .join("test_wavs")
                .join("0.wav"),
        ),
    };

    println!("== machine ==");
    println!(
        "cores={} asr_thread_count={} turbo_supported={}",
        std::thread::available_parallelism().map(|c| c.get()).unwrap_or(0),
        asr_thread_count(),
        turbo_supported()
    );
    report("baseline (idle)");

    let work = std::env::temp_dir().join("bea-resource-probe");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();

    // Bundled ffmpeg sidecar lives next to the release binary.
    let release_dir = std::env::current_exe()
        .unwrap()
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .expect("exe parent");
    let ffmpeg = release_dir.join("ffmpeg.exe");
    let ffprobe = release_dir.join("ffprobe.exe");
    let pipeline = FfmpegPipeline {
        ffmpeg: ffmpeg.clone(),
        ffprobe,
    };

    println!("== pipeline: fixture + chunk grid (150 s, 28 s chunks) ==");
    let source = work.join("source.wav");
    make_long_fixture(&ffmpeg, &fixture, &work.join("source.wav")).expect("fixture");
    let derived = work.join("derived");
    let started = Instant::now();
    let chunks = prepare_imported_media_chunks(&source, &MediaKind::Audio, &derived, &pipeline)
        .expect("prepare chunks");
    println!(
        "prepare_imported_media_chunks: {} chunks in {:.1}s",
        chunks.len(),
        started.elapsed().as_secs_f32()
    );
    report("after pipeline (ffmpeg children should be gone)");

    println!("== diarize-first ordering (small models dropped before the ASR session) ==");
    let audio_path = derived.join("audio.wav");
    let started = Instant::now();
    let turns = read_wav_samples_resampled_capped(&audio_path, 16_000, 3600)
        .and_then(|(samples, _)| {
            run_at_below_normal(|| {
                SpeakerDiarizer::from_models_dir(&models_root)
                    .and_then(|diarizer| diarizer.process_wave(&samples))
            })
        })
        .unwrap_or_default();
    println!(
        "diarization: {} speaker turns in {:.1}s (models dropped right after)",
        turns.len(),
        started.elapsed().as_secs_f32()
    );
    report("after diarization (diarizer models released)");

    println!("== GPU (DirectML) fail-safe on the CPU-only runtime ==");
    let gpu_result = load_asr_engine(&models_root, &engine_id, AsrDevice::DirectML);
    match gpu_result {
        Ok(_) => println!(
            "DirectML request: session built; requested device = {}, runtime ships DirectML = {} (sherpa downgrades to CPU on the CPU-only build — see its stderr line above)",
            active_asr_device().label(),
            bea_core::directml_runtime_available()
        ),
        Err(error) => println!(
            "DirectML request failed outright ({error}); active device = {} (graceful CPU fallback)",
            active_asr_device().label()
        ),
    }

    println!("== engine load (int8 smallest-match) ==");
    let started = Instant::now();
    let engine_one: ConfiguredAsrEngine =
        load_asr_engine(&models_root, &engine_id, AsrDevice::Cpu).expect("engine 1");
    println!("engine 1 loaded in {:.1}s", started.elapsed().as_secs_f32());
    report("after ASR engine load");

    let database_path = work.join("probe.db");
    let database = open_database(&database_path).expect("db");
    database
        .execute(
            "INSERT INTO meetings(id,title,status,created_at) VALUES ('probe','probe','processing','now')",
            [],
        )
        .expect("seed probe meeting");

    println!("== speed: sequential vs turbo (2 engines) ==");
    let started = Instant::now();
    transcribe_chunks_with_progress(
        &database,
        "probe",
        &chunks,
        &TranscriptLanguage::Auto,
        engine_one.clone(),
        |_, completed, total| {
            if completed == total {
                println!("  sequential chunk {completed}/{total}");
            }
        },
    )
    .expect("sequential transcription");
    let sequential = started.elapsed().as_secs_f32();
    println!("sequential: {sequential:.1}s for {} chunks", chunks.len());
    report("after sequential run (before trim)");

    trim_process_memory();
    report("after trim_process_memory (should drop)");

    // Fresh slate for the parallel run: no markers, no segments.
    clear_transcript(&database, "probe").unwrap();
    clear_transcription_progress(&database, "probe").unwrap();
    let engine_two: ConfiguredAsrEngine =
        load_asr_engine(&models_root, &engine_id, AsrDevice::Cpu).expect("engine 2");
    let started = Instant::now();
    transcribe_chunks_multi_engine_with_progress(
        &database,
        "probe",
        &chunks,
        &TranscriptLanguage::Auto,
        vec![engine_two, engine_one],
        |_, completed, total| {
            if completed == total {
                println!("  turbo chunk {completed}/{total}");
            }
        },
    )
    .expect("parallel transcription");
    let parallel = started.elapsed().as_secs_f32();
    println!(
        "turbo (2 decoders): {parallel:.1}s — {:.2}x vs sequential",
        sequential / parallel.max(0.001)
    );
    report("after turbo run (2 model sessions)");

    trim_process_memory();
    report("after trim");

    if engine_id == "qwen-standard" {
        // KV budget smoke: the 1024-token trim must not degrade output.
        let engine = Qwen3AsrEngine::from_model_dir(&engine_dir).expect("qwen");
        let chunk = AudioChunkInput {
            path: PathBuf::from(
                engine_dir
                    .join("sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25")
                    .join("test_wavs")
                    .join("fast1.wav"),
            ),
            start_seconds: 0,
            end_seconds: 60,
        };
        let result = engine.transcribe(&chunk, &TranscriptLanguage::English).expect("qwen decode");
        println!("== qwen KV=1024 smoke: text = {:?} ==", &result.text[..result.text.len().min(80)]);
    }
    println!("smoke probe complete");
}