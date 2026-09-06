import type { Meeting } from './types';

const STORAGE_KEY = 'bea.meetings.v1';

export function loadMeetings(storage: Pick<Storage, 'getItem'> = window.localStorage): Meeting[] {
  try {
    const raw = storage.getItem(STORAGE_KEY);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter(isMeeting);
  } catch {
    return [];
  }
}

export function saveMeetings(meetings: Meeting[], storage: Pick<Storage, 'setItem'> = window.localStorage): void {
  storage.setItem(STORAGE_KEY, JSON.stringify(meetings));
}

function isMeeting(value: unknown): value is Meeting {
  if (!value || typeof value !== 'object') return false;
  const meeting = value as Partial<Meeting>;
  return typeof meeting.id === 'string' && typeof meeting.title === 'string' && typeof meeting.created_at === 'string' && typeof meeting.duration_seconds === 'number' && ['draft', 'recording', 'paused', 'processing', 'ready', 'failed'].includes(meeting.status ?? '') && ['auto', 'en', 'fil', 'taglish'].includes(meeting.language ?? '');
}
