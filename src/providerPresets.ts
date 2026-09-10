import type { LocalServerPreset, ProviderConfig, ProviderKind } from './types';

export const LOCAL_PRESETS: { id: LocalServerPreset; label: string; base_url: string; models_hint: string }[] = [
  { id: 'lm-studio', label: 'LM Studio', base_url: 'http://localhost:1234/v1', models_hint: 'Load the model in LM Studio first' },
  { id: 'ollama', label: 'Ollama', base_url: 'http://localhost:11434/v1', models_hint: 'Run `ollama pull` for a model first' },
  { id: 'llamacpp', label: 'llama.cpp server', base_url: 'http://localhost:8080/v1', models_hint: 'Start llama-server with --port 8080' },
];

/// Shared dropdown options so Setup and Settings render the identical picker.
export const PROVIDER_KIND_OPTIONS: Array<{ value: ProviderKind; label: string }> = [
  { value: 'OpenRouter', label: 'OpenRouter' },
  { value: 'OpenAiCompatible', label: 'OpenAI-compatible' },
  { value: 'ClaudeCompatible', label: 'Claude-compatible proxy' },
  { value: 'Local', label: 'Local server (LM Studio / Ollama / llama.cpp)' },
  { value: 'OpenAiOAuth', label: 'OpenAI (ChatGPT sign-in)' },
];

/// Kind-switch contract used by Setup and Settings alike: reset the base URL
/// for the new kind and prefill a valid Codex minutes model (the ChatGPT
/// picker otherwise renders its first option while the state stays empty).
export const withProviderKind = (provider: ProviderConfig, kind: ProviderKind): ProviderConfig => ({
  ...provider,
  kind,
  base_url: DEFAULT_BASE_URLS[kind] ?? '',
  model: kind === 'OpenAiOAuth' && !provider.model ? CODEX_FALLBACK_MODELS[0] : provider.model,
});

export const DEFAULT_BASE_URLS: Record<ProviderKind, string> = {
  OpenRouter: 'https://openrouter.ai/api/v1',
  OpenAiCompatible: '',
  ClaudeCompatible: '',
  Local: 'http://localhost:1234/v1',
  OpenAiOAuth: 'https://chatgpt.com/backend-api/codex',
};

// Static fallback for the Codex backend; codex_list_models_command refreshes
// this from the account's real catalog when it responds. Kept in sync with
// the models the ChatGPT/Codex backend currently serves — older slugs
// (gpt-5.1-codex etc.) now answer HTTP 400 "model is not supported when
// using Codex with a ChatGPT account".
export const CODEX_FALLBACK_MODELS = [
  'gpt-5.6-sol',
  'gpt-5.6-terra',
  'gpt-5.6-luna',
  'gpt-5.5',
  'gpt-5.4',
  'gpt-5.4-mini',
  'gpt-5.3-codex',
  'gpt-5.3-codex-spark',
];

export const presetBaseUrl = (kind: ProviderKind, preset: LocalServerPreset): string =>
  kind === 'Local'
    ? LOCAL_PRESETS.find((entry) => entry.id === preset)?.base_url ?? DEFAULT_BASE_URLS[kind]
    : DEFAULT_BASE_URLS[kind];

/// OpenAI OAuth (ChatGPT sign-in) holds no API key, so Setup/Settings edits
/// (minutes model, base url) are safe to persist immediately. Keyed providers
/// keep the explicit test/save flow so the key is stored with the config —
/// and the OAuth kind shows no save button at all, which previously made
/// model changes silently vanish until a full re-login.
export const providerPersistsImmediately = (kind: ProviderKind): boolean =>
  kind === 'OpenAiOAuth';

export const providerLabel = (kind: ProviderKind): string =>
  kind === 'OpenRouter' ? 'OpenRouter'
  : kind === 'ClaudeCompatible' ? 'Claude-compatible proxy'
  : kind === 'OpenAiCompatible' ? 'OpenAI-compatible'
  : kind === 'Local' ? 'Local server'
  : 'OpenAI (ChatGPT sign-in)';

/// Chat model options for the ChatGPT sign-in kind. codex_list_models_command
/// answers [] when the backend has no public catalog (or errors) — that is the
/// signal to use the static fallback list, never an empty picker.
export const codexModelOptionsOrFallback = (models: string[]): string[] =>
  models.length ? models : CODEX_FALLBACK_MODELS;
