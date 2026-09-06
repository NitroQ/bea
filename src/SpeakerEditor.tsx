import { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { SpeakerName } from './types';
import Icon from './Icon';

type Props = {
  meetingId: string;
  names: SpeakerName[];
  onNamesChanged: (names: SpeakerName[]) => void;
  onNotice: (message: string) => void;
  onClose: () => void;
};

export default function SpeakerEditor({ meetingId, names, onNamesChanged, onNotice, onClose }: Props) {
  const [drafts, setDrafts] = useState<Record<number, string>>({});
  const nextIndex = names.length ? Math.max(...names.map((n) => n.speaker_index)) + 1 : 0;

  async function save(index: number) {
    const name = (drafts[index] ?? '').trim();
    if (!name) return;
    try {
      await invoke('set_speaker_name_command', { meetingId: meetingId, speakerIndex: index, name });
      const updated = names.some((n) => n.speaker_index === index)
        ? names.map((n) => (n.speaker_index === index ? { ...n, name } : n))
        : [...names, { speaker_index: index, name }].sort((a, b) => a.speaker_index - b.speaker_index);
      onNamesChanged(updated);
      onNotice(`Speaker ${index + 1} named “${name}”.`);
    } catch (error) {
      onNotice(`Could not save the speaker name: ${String(error)}`);
    }
  }

  return <div className="modal-backdrop" role="presentation" onMouseDown={(event) => event.target === event.currentTarget && onClose()}><div className="modal compact-modal"><div className="modal-heading"><div><span className="page-kicker">Manual assignment</span><h2>Speakers</h2><p>Name the people in this meeting, then assign transcript blocks to them from the transcript view.</p></div><button className="icon-button" onClick={onClose} aria-label="Close"><Icon name="x" size={17} /></button></div><div className="speaker-editor-rows">{names.length === 0 && <p className="inspector-muted">No speakers yet — add the first one below.</p>}{names.map((entry) => <div className="speaker-editor-row" key={entry.speaker_index}><span className="conversation-speaker assigned">Speaker {entry.speaker_index + 1}</span><input value={drafts[entry.speaker_index] ?? entry.name} onChange={(event) => setDrafts((current) => ({ ...current, [entry.speaker_index]: event.target.value }))} onKeyDown={(event) => event.key === 'Enter' && void save(entry.speaker_index)} placeholder="e.g. Maria Santos" aria-label={`Name for speaker ${entry.speaker_index + 1}`} /><button className="secondary small" onClick={() => void save(entry.speaker_index)}>Save</button></div>)}</div><div className="modal-actions"><button className="secondary" onClick={() => setDrafts((current) => ({ ...current, [nextIndex]: `Speaker ${nextIndex + 1}` }))}>Add speaker</button><button className="primary" onClick={onClose}>Done</button></div></div></div>;
}
