// Accuracy probe: transcribe the model's own ground-truth fixtures and compute
// WER against transcript.txt. Usage:
//   cargo run --release --example accuracy_probe -- <model_dir> <test_wavs_dir> [taglish|english|filipino]
use bea_core::{AsrEngine, AudioChunkInput, Qwen3AsrEngine, TranscriptLanguage};

/// Simple token-level WER (Levenshtein over word sequences).
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

/// Strips CJK punctuation and other non-speech tokens for a fairer WER.
fn normalize(text: &str) -> String {
    text.replace(
        ['，', '。', '、', '！', '？', ',', '.', '!', '?', ';', ':'],
        " ",
    )
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let model_dir = args
        .get(1)
        .expect("usage: <model_dir> <test_wavs_dir> [language]");
    let wavs_dir = args
        .get(2)
        .expect("usage: <model_dir> <test_wavs_dir> [language]");
    let language = match args.get(3).map(String::as_str) {
        Some("english") => TranscriptLanguage::English,
        Some("taglish") => TranscriptLanguage::Taglish,
        Some("filipino") => TranscriptLanguage::Filipino,
        _ => TranscriptLanguage::Auto,
    };
    // Parse ground truth: "<file>.wav <expected text>" per line.
    let truth_text =
        std::fs::read_to_string(format!("{wavs_dir}/transcript.txt")).expect("transcript.txt");
    let mut cases = Vec::new();
    for line in truth_text.lines() {
        let Some((file, expected)) = line.split_once(' ') else {
            continue;
        };
        cases.push((file.trim().to_string(), expected.trim().to_string()));
    }
    println!(
        "loaded {} ground-truth cases; engine language hint: {:?}",
        cases.len(),
        language
    );
    let engine = Qwen3AsrEngine::from_model_dir(model_dir).expect("engine");
    let mut total_wer = 0.0;
    let mut scored = 0;
    for (index, (file, expected)) in cases.iter().enumerate() {
        let path = format!("{wavs_dir}/{file}");
        let chunk = AudioChunkInput {
            path: path.clone().into(),
            start_seconds: 0,
            end_seconds: 60,
        };
        let start = std::time::Instant::now();
        let result = match engine.transcribe(&chunk, &language) {
            Ok(result) => result,
            Err(error) => {
                println!("[{index}] {file}: ERROR {error}");
                continue;
            }
        };
        let elapsed = start.elapsed().as_secs_f32();
        let score = wer(&normalize(expected), &normalize(&result.text));
        total_wer += score;
        scored += 1;
        println!(
            "[{index}] {file}: WER {:.1}% ({elapsed:.1}s)\n  expected: {}\n  got:      {}",
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
