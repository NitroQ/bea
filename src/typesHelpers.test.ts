import { describe, expect, it } from 'vitest';
import {
  REASONING_EFFORTS,
  SILENCE_TEXT,
  exportExtension,
  isSilenceSegment,
  mediaKindIsVideo,
  minutesHasExportableContent,
  normalizeReasoningEffort,
  speakerLabel,
  type Minutes,
  type TranscriptSegment,
} from './types';

const seg = (text: string): TranscriptSegment => ({
  id: 's1', meeting_id: 'm1', start_seconds: 0, end_seconds: 5, text,
});

describe('normalizeReasoningEffort', () => {
  it('accepts low/medium/high, rejects everything else', () => {
    for (const v of ['low', 'medium', 'high']) expect(normalizeReasoningEffort(v)).toBe(v);
    for (const v of ['off', '', 'x', 'OFF', 'LOW ']) expect(normalizeReasoningEffort(v)).toBe('off');
  });
  it('exposes all four efforts with copy', () => {
    expect(REASONING_EFFORTS.map((e) => e.id)).toEqual(['off', 'low', 'medium', 'high']);
    expect(REASONING_EFFORTS.every((e) => e.label && e.hint)).toBe(true);
  });
});

describe('isSilenceSegment', () => {
  it('matches only the silent marker, not lookalikes', () => {
    expect(isSilenceSegment(seg(SILENCE_TEXT))).toBe(true);
    expect(isSilenceSegment(seg('  [silence]  '))).toBe(true);
    expect(isSilenceSegment(seg('a [silence] b'))).toBe(false);
    expect(isSilenceSegment(seg(''))).toBe(false);
  });
});

describe('mediaKindIsVideo', () => {
  it('treats audio kinds as audio, everything else as video', () => {
    expect(mediaKindIsVideo('audio')).toBe(false);
    expect(mediaKindIsVideo('Audio')).toBe(false);
    expect(mediaKindIsVideo('video')).toBe(true);
    expect(mediaKindIsVideo('Video')).toBe(true);
    expect(mediaKindIsVideo(null)).toBe(true);
    expect(mediaKindIsVideo(undefined)).toBe(true);
  });
});

describe('speakerLabel', () => {
  const names = [
    { speaker_index: 0, name: 'Ana' },
    { speaker_index: 1, name: 'Ben' },
  ];
  it('uses the name when known, a positional label otherwise', () => {
    expect(speakerLabel(0, names)).toBe('Ana');
    expect(speakerLabel(2, names)).toBe('Speaker 3');
    expect(speakerLabel(null, names)).toBe('Speaker —');
  });
  it('appends extra speakers after the primary', () => {
    expect(speakerLabel(0, names, [1])).toBe('Ana + Ben');
    expect(speakerLabel(0, names, [0, 1])).toBe('Ana + Ben');
  });
});

describe('minutesHasExportableContent', () => {
  const empty: Minutes = {
    title: '', summary: '', agenda: [],
    decisions: [], action_items: [], unresolved: [],
  };
  it('is true when any meaningful section is present', () => {
    expect(minutesHasExportableContent(empty)).toBe(false);
    expect(minutesHasExportableContent({ ...empty, summary: 'did a thing' })).toBe(true);
    expect(minutesHasExportableContent({ ...empty, action_items: [{ kind: 'a', summary: 'x', confidence: 1, evidence: [] }] })).toBe(true);
    expect(minutesHasExportableContent({ ...empty, summary: '   ' })).toBe(false);
  });
});

describe('exportExtension', () => {
  it('maps markdown to md, pdf/docx through', () => {
    expect(exportExtension('markdown')).toBe('md');
    expect(exportExtension('pdf')).toBe('pdf');
    expect(exportExtension('docx')).toBe('docx');
  });
});
