// Probe: transcribe a WAV chunk with the Whisper compatibility engine and
// print the raw result. Usage:
//   transcribe_probe_whisper.exe <model_dir> <wav> [taglish|english|filipino]
use bea_core::{AsrEngine, AudioChunkInput, TranscriptLanguage, WhisperCompatibilityEngine};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let model_dir = args.get(1).expect("model dir");
    let wav = args.get(2).expect("wav path");
    let language = match args.get(3).map(String::as_str) {
        Some("english") => TranscriptLanguage::English,
        Some("taglish") => TranscriptLanguage::Taglish,
        Some("filipino") => TranscriptLanguage::Filipino,
        _ => TranscriptLanguage::Auto,
    };
    let engine = WhisperCompatibilityEngine::from_model_dir(model_dir).expect("engine");
    let chunk = AudioChunkInput {
        path: wav.into(),
        start_seconds: 0,
        end_seconds: 600,
    };
    let start = std::time::Instant::now();
    let result = engine.transcribe(&chunk, &language).expect("transcribe");
    println!("elapsed: {:.1}s", start.elapsed().as_secs_f32());
    println!("text: {:?}", result.text);
}
