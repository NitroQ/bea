import { describe, expect, it } from 'vitest';
import { parseClarificationSuggestions, parseSlashCommand, parseVisionSuffix } from './chatActions';

describe('slash command parsing', () => {
  it('parses each supported action with its argument', () => {
    expect(parseSlashCommand('/correction Hans=>Hansel')).toEqual({ action: 'correction', arg: 'Hans=>Hansel' });
    expect(parseSlashCommand('/clarify the vote was 5-2')).toEqual({ action: 'clarify', arg: 'the vote was 5-2' });
    expect(parseSlashCommand('/context Q3 budget is frozen')).toEqual({ action: 'context', arg: 'Q3 budget is frozen' });
    expect(parseSlashCommand('/custom')).toEqual({ action: 'custom', arg: '' });
  });
  it('treats plain text as a question', () => {
    expect(parseSlashCommand('who owned the hiring decision?')).toEqual({ action: null, arg: 'who owned the hiring decision?' });
  });
  it('rejects unknown slashes as questions, not silent failures', () => {
    expect(parseSlashCommand('/nope thing')).toEqual({ action: 'unknown:/nope', arg: 'thing' });
  });
});

describe('/vision suffix parsing', () => {
  it('extracts frame seconds and strips the suffix from the question', () => {
    expect(parseVisionSuffix('What is on the slide? /vision 320, 480')).toEqual({ question: 'What is on the slide?', includeFrames: [320, 480] });
    expect(parseVisionSuffix('Describe the whiteboard /vision 12')).toEqual({ question: 'Describe the whiteboard', includeFrames: [12] });
  });
  it('is case-insensitive and tolerates spaces around values', () => {
    expect(parseVisionSuffix('What changed? /VISION 5 , 9')).toEqual({ question: 'What changed?', includeFrames: [5, 9] });
  });
  it('passes plain questions through without frames', () => {
    expect(parseVisionSuffix('who owned the hiring decision?')).toEqual({ question: 'who owned the hiring decision?', includeFrames: undefined });
  });
  it('treats a suffix with non-numeric junk as no match (original behavior)', () => {
    expect(parseVisionSuffix('Read the chart /vision abc')).toEqual({ question: 'Read the chart /vision abc', includeFrames: undefined });
    expect(parseVisionSuffix('Read the chart /vision -3')).toEqual({ question: 'Read the chart /vision -3', includeFrames: undefined });
  });
});

describe('clarification suggestions', () => {
  it('round-trips suggestions with options and free text', () => {
    const payload = JSON.stringify({ questions: [{ question: 'Who chaired the meeting?', options: ['Maria Santos', 'John Cruz'] }] });
    const parsed = parseClarificationSuggestions(payload);
    expect(parsed).toEqual([{ question: 'Who chaired the meeting?', options: ['Maria Santos', 'John Cruz'] }]);
  });
  it('returns an empty list for malformed model output instead of throwing', () => {
    expect(parseClarificationSuggestions('not json')).toEqual([]);
    expect(parseClarificationSuggestions('{"questions": "nope"}')).toEqual([]);
  });
});
