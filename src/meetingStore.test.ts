import { describe, expect, it } from 'vitest';
import { loadMeetings, saveMeetings } from './meetingStore';
import type { Meeting } from './types';

function memoryStorage() {
  const values = new Map<string, string>();
  return { getItem: (key: string) => values.get(key) ?? null, setItem: (key: string, value: string) => values.set(key, value) };
}

describe('meeting persistence', () => {
  it('round-trips valid meetings and ignores corrupt records', () => {
    const storage = memoryStorage();
    const meeting: Meeting = { id: 'm1', title: 'Planning', status: 'draft', duration_seconds: 0, language: 'taglish', created_at: new Date().toISOString() };
    saveMeetings([meeting], storage);
    expect(loadMeetings(storage)).toEqual([meeting]);
    storage.setItem('bea.meetings.v1', '[{"title":"missing fields"}]');
    expect(loadMeetings(storage)).toEqual([]);
  });
});
