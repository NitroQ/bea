import { describe, expect, it } from 'vitest';
import { MIN_CONTEXT_TOKENS, usableContextTokens } from './modelContext';

describe('usableContextTokens', () => {
  it('keeps the windows providers actually publish', () => {
    expect(usableContextTokens(8_192)).toBe(8_192);
    expect(usableContextTokens(256_000)).toBe(256_000);
    expect(usableContextTokens(500_000)).toBe(500_000);
    expect(usableContextTokens(1_000_000)).toBe(1_000_000);
    expect(usableContextTokens(1_300_000)).toBe(1_300_000);
  });

  it('drops windows a local discovery cannot report', () => {
    // Ollama/LM Studio/llama.cpp publish model ids, not the loaded context.
    expect(usableContextTokens(null)).toBeNull();
    expect(usableContextTokens(undefined)).toBeNull();
  });

  it('rejects values that are not real context windows', () => {
    expect(usableContextTokens(0)).toBeNull();
    expect(usableContextTokens(-1)).toBeNull();
    expect(usableContextTokens(Number.NaN)).toBeNull();
    expect(usableContextTokens(Number.POSITIVE_INFINITY)).toBeNull();
    // A typo must not become the budget for a model.
    expect(usableContextTokens(MIN_CONTEXT_TOKENS - 1)).toBeNull();
  });

  it('rounds rather than storing a fractional window', () => {
    expect(usableContextTokens(32_768.4)).toBe(32_768);
  });
});
