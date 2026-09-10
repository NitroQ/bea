import { describe, expect, it } from 'vitest';
import { exportExtension, mediaKindIsVideo, minutesHasExportableContent, statusLabel } from './types';
import type { Minutes } from './types';

describe('meeting status presentation', () => {
  it('keeps processing and failure states visible to the reviewer', () => {
    expect(statusLabel.processing).toBe('Processing');
    expect(statusLabel.failed).toBe('Needs attention');
  });
});

describe('media kind player selection', () => {
  it('treats every audio spelling as audio, regardless of stored casing', () => {
    // The DB stores capitalized kinds ('Audio'/'Video'); a case-sensitive
    // check rendered audio meetings in a black <video> box.
    expect(mediaKindIsVideo('Audio')).toBe(false);
    expect(mediaKindIsVideo('audio')).toBe(false);
    expect(mediaKindIsVideo('VIDEO')).toBe(true);
    expect(mediaKindIsVideo('Video')).toBe(true);
    expect(mediaKindIsVideo(undefined)).toBe(true);
    expect(mediaKindIsVideo(null)).toBe(true);
  });
});

describe('minutes export guards', () => {
  const emptyMinutes: Minutes = { title: '', summary: '', agenda: [], decisions: [], action_items: [], unresolved: [] };

  it('refuses to export minutes that have no generated content', () => {
    // Exporting an empty shell produced blank documents that looked like a
    // successful share — the button must explain itself instead.
    expect(minutesHasExportableContent(emptyMinutes)).toBe(false);
    expect(minutesHasExportableContent({ ...emptyMinutes, summary: 'Discussed budget.' })).toBe(true);
    expect(minutesHasExportableContent({ ...emptyMinutes, decisions: [{ kind: 'decision', summary: 'x', confidence: 1, evidence: [] }] })).toBe(true);
    expect(minutesHasExportableContent({ ...emptyMinutes, action_items: [{ kind: 'action', summary: 'y', confidence: 1, evidence: [] }] })).toBe(true);
    expect(minutesHasExportableContent({ ...emptyMinutes, agenda: [{ heading: 'Opening' }] })).toBe(true);
  });

  it('maps the markdown format to the md extension the backend expects', () => {
    // The save dialog must propose ".md" — the exporter rejects ".markdown".
    expect(exportExtension('markdown')).toBe('md');
    expect(exportExtension('pdf')).toBe('pdf');
    expect(exportExtension('docx')).toBe('docx');
  });
});
