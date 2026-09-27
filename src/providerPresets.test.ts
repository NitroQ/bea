import { describe, expect, it } from 'vitest';
import {
  CODEX_FALLBACK_MODELS,
  DEFAULT_BASE_URLS,
  DEFAULT_PROVIDER_KIND,
  LOCAL_PRESETS,
  PROVIDER_KIND_OPTIONS,
  codexModelOptionsOrFallback,
  presetBaseUrl,
  providerLabel,
  providerPersistsImmediately,
  withProviderKind,
} from './providerPresets';
import type { ProviderConfig } from './types';

const base: ProviderConfig = {
  id: 'primary',
  kind: 'OpenRouter',
  base_url: 'https://openrouter.ai/api/v1',
  model: 'qwen/qwen3.7-flash',
  enabled: true,
  reasoning_effort: 'low',
};

describe('DEFAULT_BASE_URLS', () => {
  it('prefills a real endpoint for every non-OAuth kind', () => {
    expect(DEFAULT_BASE_URLS.OpenRouter).toContain('openrouter.ai');
    expect(DEFAULT_BASE_URLS.Local).toMatch(/^http:\/\/localhost:1234/);
    expect(DEFAULT_BASE_URLS.OpenAiOAuth).toContain('chatgpt.com');
  });
});

describe('withProviderKind', () => {
  it('resets the base URL when the kind changes', () => {
    const next = withProviderKind(base, 'Local');
    expect(next.kind).toBe('Local');
    expect(next.base_url).toBe(DEFAULT_BASE_URLS.Local);
  });
  it('prefills a valid Codex model only for the OAuth kind', () => {
    const empty: ProviderConfig = { ...base, model: '' };
    expect(withProviderKind(empty, 'OpenAiOAuth').model).toBe(CODEX_FALLBACK_MODELS[0]);
    expect(withProviderKind(empty, 'OpenRouter').model).toBe('');
  });
  it('preserves an existing model when switching kinds', () => {
    expect(withProviderKind(base, 'OpenAiCompatible').model).toBe(base.model);
  });
  it('clears a Codex slug when leaving ChatGPT sign-in', () => {
    // No other provider serves these ids, so carrying one over would leave a
    // model the connection test can only reject.
    const oauth: ProviderConfig = { ...base, kind: 'OpenAiOAuth', model: CODEX_FALLBACK_MODELS[0] };
    expect(withProviderKind(oauth, 'OpenRouter').model).toBe('');
  });
});

describe('DEFAULT_PROVIDER_KIND', () => {
  it('starts a new workspace on ChatGPT sign-in, listed first', () => {
    expect(DEFAULT_PROVIDER_KIND).toBe('OpenAiOAuth');
    expect(PROVIDER_KIND_OPTIONS[0].value).toBe('OpenAiOAuth');
  });
});

describe('presetBaseUrl', () => {
  it('returns the local preset URL only when the kind is Local', () => {
    const ollama = LOCAL_PRESETS.find(p => p.id === 'ollama')!;
    expect(presetBaseUrl('Local', 'ollama')).toBe(ollama.base_url);
    expect(presetBaseUrl('OpenRouter', 'ollama')).toBe(DEFAULT_BASE_URLS.OpenRouter);
  });
  it('falls back to the kind default for an unknown preset', () => {
    expect(presetBaseUrl('Local', 'nope' as any)).toBe(DEFAULT_BASE_URLS.Local);
  });
});

describe('providerLabel', () => {
  it('renders a human label for every kind', () => {
    expect(providerLabel('OpenRouter')).toBe('OpenRouter');
    expect(providerLabel('Local')).toBe('Local server');
    expect(PROVIDER_KIND_OPTIONS).toHaveLength(5);
  });
});

describe('providerPersistsImmediately', () => {
  it('is true only for the OAuth kind (which holds no API key)', () => {
    expect(providerPersistsImmediately('OpenAiOAuth')).toBe(true);
    expect(providerPersistsImmediately('OpenRouter')).toBe(false);
  });
});

describe('codexModelOptionsOrFallback', () => {
  it('keeps a live catalog when present', () => {
    expect(codexModelOptionsOrFallback(['a', 'b'])).toEqual(['a', 'b']);
  });
  it('falls back to the static list when the catalog is empty', () => {
    expect(codexModelOptionsOrFallback([])).toEqual(CODEX_FALLBACK_MODELS);
  });
});
