import type { LocalServerPreset, ProviderKind } from './types';

export const LOCAL_PRESETS: { id: LocalServerPreset; label: string; base_url: string; models_hint: string }[] = [
  { id: 'lm-studio', label: 'LM Studio', base_url: 'http://localhost:1234/v1', models_hint: 'Load the model in LM Studio first' },
  { id: 'ollama', label: 'Ollama', base_url: 'http://localhost:11434/v1', models_hint: 'Run `ollama pull` for a model first' },
  { id: 'llamacpp', label: 'llama.cpp server', base_url: 'http://localhost:8080/v1', models_hint: 'Start llama-server with --port 8080' },
];

export const DEFAULT_BASE_URLS: Record<ProviderKind, string> = {
  OpenRouter: 'https://openrouter.ai/api/v1',
  OpenAiCompatible: '',
  ClaudeCompatible: '',
  Local: 'http://localhost:1234/v1',
  OpenAiOAuth: 'https://chatgpt.com/backend-api/codex',
};

// Static fallback for the Codex backend; codex_list_models_command refreshes
// this from the account's real catalog when it responds.
export const CODEX_FALLBACK_MODELS = ['gpt-5.1-codex', 'gpt-5.1-codex-mini', 'gpt-5.1'];

export const presetBaseUrl = (kind: ProviderKind, preset: LocalServerPreset): string =>
  kind === 'Local'
    ? LOCAL_PRESETS.find((entry) => entry.id === preset)?.base_url ?? DEFAULT_BASE_URLS[kind]
    : DEFAULT_BASE_URLS[kind];

export const providerLabel = (kind: ProviderKind): string =>
  kind === 'OpenRouter' ? 'OpenRouter'
  : kind === 'ClaudeCompatible' ? 'Claude-compatible proxy'
  : kind === 'OpenAiCompatible' ? 'OpenAI-compatible'
  : kind === 'Local' ? 'Local server'
  : 'OpenAI (ChatGPT sign-in)';
