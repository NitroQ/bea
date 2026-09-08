// Probe: transcribe WAVs with the real Whisper compatibility engine and
// compute WER against a ground-truth transcript file (same format as the
// sherpa test_wavs/transcript.txt: "<file>.wav <expected text>" per line).
// Usage:
//   accuracy_probe_whisper.exe <model_dir> <transcript_file> [language]
use bea_core::{AsrEngine, AudioChunkInput, TranscriptLanguage, WhisperCompatibilityEngine};

fn wer(reference: &str, hypothesis: &str) -> f64 {
    let tokenize = |text: &str| -> Vec<String> {
        text.to_lowercase()
            .split(|ch: char| !(ch.is_alphanumeric() || ch == '\''))
            .filter(|word| !word.is_empty())
            .map(|word| word.to_string())
            .collect()
    };
    let reference = tokenize(reference);
    let hypothesis = tokenize(hypothesis);
    if reference.is_empty() {
        return if hypothesis.is_empty() { 0.0 } else { 1.0 };
    }
    let mut previous: Vec<usize> = (0..=hypothesis.len()).collect();
    for (ref_index, ref_word) in reference.iter().enumerate() {
        let mut current = vec![ref_index + 1];
        for (hyp_index, hyp_word) in hypothesis.iter().enumerate() {
            let substitution = previous[hyp_index] + usize::from(ref_word != hyp_word);
            let insertion = previous[hyp_index + 1] + 1;
            let deletion = current[hyp_index] + 1;
            current.push(substitution.min(insertion).min(deletion));
        }
        previous = current;
    }
    previous[hypothesis.len()] as f64 / reference.len() as f64
}

fn normalize(text: &str) -> String {
    text.replace(
        [
            '，', '。', '、', '！', '？', ',', '.', '!', '?', ';', ':', '"',
        ],
        " ",
    )
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let model_dir = args
        .get(1)
        .expect("usage: <model_dir> <transcript_file> [lang]");
    let truth_path = args
        .get(2)
        .expect("usage: <model_dir> <transcript_file> [lang]");
    let language = match args.get(3).map(String::as_str) {
        Some("english") => TranscriptLanguage::English,
        Some("taglish") => TranscriptLanguage::Taglish,
        Some("filipino") => TranscriptLanguage::Filipino,
        _ => TranscriptLanguage::Auto,
    };
    let engine = WhisperCompatibilityEngine::from_model_dir(model_dir).expect("engine");
    println!("whisper engine loaded; language hint: {:?}", language);
    let truth_text = std::fs::read_to_string(truth_path).expect("transcript file");
    let mut total_wer = 0.0;
    let mut scored = 0;
    for line in truth_text.lines() {
        let Some((file, expected)) = line.split_once(' ') else {
            continue;
        };
        // Resolve fixture names relative to the model's test_wavs directory
        // (trans.txt lists bare names like "0.wav").
        let path = std::path::Path::new(file)
            .file_name()
            .map(|name| format!("{}/test_wavs/{}", model_dir, name.to_string_lossy()))
            .unwrap_or_else(|| file.to_string());
        let chunk = AudioChunkInput {
            path: path.clone().into(),
            start_seconds: 0,
            end_seconds: 600,
        };
        let start = std::time::Instant::now();
        let result = match engine.transcribe(&chunk, &language) {
            Ok(result) => result,
            Err(error) => {
                println!("{file}: ERROR {error}");
                continue;
            }
        };
        let elapsed = start.elapsed().as_secs_f32();
        let score = wer(&normalize(expected), &normalize(&result.text));
        total_wer += score;
        scored += 1;
        println!(
            "{file}: WER {:.1}% ({elapsed:.1}s)\n  expected: {}\n  got:      {}",
            score * 100.0,
            normalize(expected),
            result.text
        );
    }
    if scored > 0 {
        println!(
            "\n=== average WER across {} files: {:.1}% ===",
            scored,
            total_wer / scored as f64 * 100.0
        );
    }
}
