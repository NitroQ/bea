import { describe, expect, it } from 'vitest';
import { setupFromRuntime } from './App';
import { presetBaseUrl, providerPersistsImmediately, codexModelOptionsOrFallback } from './providerPresets';
import { modelSelectTriggerLabel } from './ModelSelect';

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

describe('provider change persistence', () => {
  it('saves keyless OAuth provider edits immediately, keyed providers on test', () => {
    // Regression: the ChatGPT sign-in kind has no visible save button, so a
    // Minutes-model change silently vanished until a full re-login.
    expect(providerPersistsImmediately('OpenAiOAuth')).toBe(true);
    expect(providerPersistsImmediately('OpenRouter')).toBe(false);
    expect(providerPersistsImmediately('OpenAiCompatible')).toBe(false);
    expect(providerPersistsImmediately('ClaudeCompatible')).toBe(false);
    expect(providerPersistsImmediately('Local')).toBe(false);
  });
});

describe('codex chat model options', () => {
  it('falls back to the static catalog when the backend returns nothing', () => {
    // codex_list_models_command returns [] when the backend has no catalog
    // (or errors) — the chat picker must show the static models then, not
    // just "Default model (Settings)" with no other choice.
    expect(codexModelOptionsOrFallback([]).length).toBeGreaterThan(3);
    expect(codexModelOptionsOrFallback([])).toContain('gpt-5.6-luna');
    expect(codexModelOptionsOrFallback(['gpt-5.6-luna', 'gpt-5.6-sol'])).toEqual(['gpt-5.6-luna', 'gpt-5.6-sol']);
  });
});

describe('model select trigger label', () => {
  it('shows "Default model (Settings)" for the empty override instead of "Select a model"', () => {
    // Regression: after picking "Default model (Settings)" in the chat picker
    // the closed trigger said "Select a model", which reads like nothing is
    // chosen even though chat correctly falls back to the Settings default.
    expect(modelSelectTriggerLabel('', true)).toBe('Default model (Settings)');
  });

  it('keeps "Select a model" where no default option exists', () => {
    expect(modelSelectTriggerLabel('', false)).toBe('Select a model');
    expect(modelSelectTriggerLabel('z-ai/glm-5.3-flash', true)).toBe('z-ai/glm-5.3-flash');
  });
});
