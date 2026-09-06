import { describe, expect, it } from 'vitest';
import { vttSummary } from './vttImport';

describe('vtt import summary', () => {
  it('counts distinct named speakers', () => {
    const summary = vttSummary([
      { start_seconds: 5, end_seconds: 8, speaker: 'Maria Santos', text: 'a' },
      { start_seconds: 8, end_seconds: 12, speaker: 'John Cruz', text: 'b' },
      { start_seconds: 60, end_seconds: 62, speaker: 'Maria Santos', text: 'c' },
    ]);
    expect(summary.cueCount).toBe(3);
    expect(summary.speakerNames).toEqual(['Maria Santos', 'John Cruz']);
  });
});
