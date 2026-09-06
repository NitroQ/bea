import { describe, expect, it } from 'vitest';
import { statusLabel } from './types';

describe('meeting status presentation', () => {
  it('keeps processing and failure states visible to the reviewer', () => {
    expect(statusLabel.processing).toBe('Processing');
    expect(statusLabel.failed).toBe('Needs attention');
  });
});
