use bea_core::{RecorderConfig, SegmentedWavRecorder};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let seconds = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "10".into())
        .parse::<u64>()?;
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or("no default microphone is available")?;
    let supported = device.default_input_config()?;
    let output = std::env::temp_dir().join(format!("bea-microphone-smoke-{}", std::process::id()));
    std::fs::create_dir_all(&output)?;
    let recorder = Arc::new(Mutex::new(SegmentedWavRecorder::new(
        &output,
        RecorderConfig {
            sample_rate: supported.sample_rate().0,
            channels: supported.channels(),
            chunk_seconds: 60,
            max_duration_seconds: 5 * 60 * 60,
        },
    )?));
    recorder.lock().unwrap().start()?;
    let config = supported.config();
    let callback_recorder = Arc::clone(&recorder);
    let error_callback = |error| eprintln!("microphone stream error: {error}");
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => device.build_input_stream(
            &config,
            move |data: &[f32], _| {
                if let Ok(mut recorder) = callback_recorder.lock() {
                    let samples: Vec<i16> = data
                        .iter()
                        .map(|sample| (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
                        .collect();
                    let _ = recorder.push_samples(&samples);
                }
            },
            error_callback,
            None,
        )?,
        cpal::SampleFormat::I16 => device.build_input_stream(
            &config,
            move |data: &[i16], _| {
                if let Ok(mut recorder) = callback_recorder.lock() {
                    let _ = recorder.push_samples(data);
                }
            },
            error_callback,
            None,
        )?,
        cpal::SampleFormat::U16 => device.build_input_stream(
            &config,
            move |data: &[u16], _| {
                if let Ok(mut recorder) = callback_recorder.lock() {
                    let samples: Vec<i16> = data
                        .iter()
                        .map(|sample| (*sample as i32 - 32768) as i16)
                        .collect();
                    let _ = recorder.push_samples(&samples);
                }
            },
            error_callback,
            None,
        )?,
        format => return Err(format!("unsupported sample format: {format:?}").into()),
    };
    stream.play()?;
    println!(
        "capturing microphone for {seconds}s at {} Hz",
        supported.sample_rate().0
    );
    std::thread::sleep(Duration::from_secs(seconds));
    drop(stream);
    let chunks = recorder.lock().unwrap().stop()?;
    if chunks.is_empty() {
        return Err("microphone capture produced no completed chunks".into());
    }
    let file_count = std::fs::read_dir(&output)?.count();
    println!(
        "captured {} final chunk(s), {} persisted file(s) under {}",
        chunks.len(),
        file_count,
        output.display()
    );
    Ok(())
}
