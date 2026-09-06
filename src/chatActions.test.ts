import { describe, expect, it } from 'vitest';
import { parseClarificationSuggestions, parseSlashCommand } from './chatActions';

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
