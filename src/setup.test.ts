import { describe, expect, it } from 'vitest';
import { setupFromRuntime } from './App';
import { presetBaseUrl } from './providerPresets';

const readyRuntime = { asr_model_available: true, ffmpeg_available: true, ffprobe_available: true, ocr_available: true, provider_configured: true, can_transcribe_locally: true };

describe('guided setup readiness', () => {
  it('keeps the library locked when any required capability is missing', () => {
    const status = setupFromRuntime({ ...readyRuntime, ocr_available: false }, 'qwen-standard', true, []);
    expect(status.complete).toBe(false);
    expect(status.tools.find((tool) => tool.id === 'tesseract')?.status).toBe('missing');
  });

  it('does not unlock media tools when FFprobe is missing from the pair', () => {
    const status = setupFromRuntime({ ...readyRuntime, ffprobe_available: false }, 'qwen-standard', true, []);
    expect(status.complete).toBe(false);
    expect(status.tools.find((tool) => tool.id === 'ffmpeg')?.detail).toContain('FFprobe');
  });

  it('requires a selected installed engine and a verified provider', () => {
    expect(setupFromRuntime(readyRuntime, 'qwen-standard', false, []).complete).toBe(false);
    expect(setupFromRuntime(readyRuntime, 'qwen-standard', true, []).complete).toBe(true);
  });

  it('supports the optional engines without changing the global gate contract', () => {
    const status = setupFromRuntime({ ...readyRuntime, asr_model_available: false }, 'nemotron-multilingual', true, ['nemotron-multilingual']);
    expect(status.engines.find((engine) => engine.id === 'nemotron-multilingual')?.status).toBe('ready');
    expect(status.complete).toBe(true);
  });
});

describe('local provider presets', () => {
  it('prefills known local server base urls', () => {
    expect(presetBaseUrl('Local', 'lm-studio')).toBe('http://localhost:1234/v1');
    expect(presetBaseUrl('Local', 'ollama')).toBe('http://localhost:11434/v1');
    expect(presetBaseUrl('Local', 'llamacpp')).toBe('http://localhost:8080/v1');
    expect(presetBaseUrl('OpenRouter', 'lm-studio')).toBe('https://openrouter.ai/api/v1');
  });
});
