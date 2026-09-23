import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { afterEach, describe, expect, it, vi } from 'vitest';
import MeetingWorkspace from './MeetingWorkspace';
import type { Meeting } from './types';

const meeting: Meeting = { id: 'timer-test', title: 'Timer test', status: 'processing', duration_seconds: 0, language: 'en', created_at: '2026-01-01T00:00:00Z' };
const noop = () => {};

function renderWorkspace(startedAt: number | null, busy = true) {
  return renderToStaticMarkup(createElement(MeetingWorkspace, {
    meeting, busy, transcriptionStartedAt: startedAt, generating: false,
    onBack: noop, onNotice: noop, onImport: noop, onImportVtt: noop,
    onRecording: noop, onMeetingStatus: noop, onRetryTranscription: noop,
    onResumeTranscription: noop, onGenerateMinutes: async () => {}, onCancelMinutesGeneration: noop,
  }));
}

afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });

describe('transcription elapsed time', () => {
  it('includes time spent away when the workspace mounts again', () => {
    vi.stubGlobal('window', {});
    vi.useFakeTimers();
    const startedAt = new Date('2026-01-01T00:00:00Z').getTime();
    vi.setSystemTime(startedAt + 12_000);
    expect(renderWorkspace(startedAt)).toContain('00:12');
    // A fresh render models returning after the workspace was unmounted.
    vi.setSystemTime(startedAt + 75_000);
    expect(renderWorkspace(startedAt)).toContain('01:15');
  });
});
