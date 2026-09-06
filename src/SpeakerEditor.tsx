import { useRef, useState } from 'react';
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
  // Added-but-unsaved rows live here so rapid "Add speaker" clicks each get a
  // distinct index (names alone would retarget the same next index).
  const [added, setAdded] = useState<number[]>([]);
  const addedCounter = useRef<number>(names.length ? Math.max(...names.map((n) => n.speaker_index)) + 1 : 0);
  const nextIndex = names.length ? Math.max(...names.map((n) => n.speaker_index)) + 1 : 0;

  function addSpeaker() {
    // Always take the highest known index + 1 so consecutive adds never share one.
    const index = Math.max(addedCounter.current, nextIndex, ...added, ...(names.length ? names.map((n) => n.speaker_index) : [0])) + 1;
    addedCounter.current = index + 1;
    setAdded((current) => [...current, index]);
    setDrafts((current) => ({ ...current, [index]: `Speaker ${index + 1}` }));
  }

  async function save(index: number) {
    const name = (drafts[index] ?? '').trim();
    if (!name) return;
    try {
      await invoke('set_speaker_name_command', { meetingId: meetingId, speakerIndex: index, name });
      const updated = names.some((n) => n.speaker_index === index)
        ? names.map((n) => (n.speaker_index === index ? { ...n, name } : n))
        : [...names, { speaker_index: index, name }].sort((a, b) => a.speaker_index - b.speaker_index);
      onNamesChanged(updated);
      setAdded((current) => current.filter((i) => i !== index));
      onNotice(`Speaker ${index + 1} named “${name}”.`);
    } catch (error) {
      onNotice(`Could not save the speaker name: ${String(error)}`);
    }
  }

  const pendingRows = added.filter((index) => !names.some((n) => n.speaker_index === index));
  return <div className="modal-backdrop" role="presentation" onMouseDown={(event) => event.target === event.currentTarget && onClose()}><div className="modal compact-modal"><div className="modal-heading"><div><span className="page-kicker">Manual assignment</span><h2>Speakers</h2><p>Name the people in this meeting, then assign transcript blocks to them from the transcript view.</p></div><button className="icon-button" onClick={onClose} aria-label="Close"><Icon name="x" size={17} /></button></div><div className="speaker-editor-rows">{names.length === 0 && pendingRows.length === 0 && <p className="inspector-muted">No speakers yet — add the first one below.</p>}{[...names.map((entry) => ({ index: entry.speaker_index, name: entry.name })), ...pendingRows.map((index) => ({ index, name: '' }))].map((row) => <div className="speaker-editor-row" key={row.index}><span className={`conversation-speaker${row.name ? ' assigned' : ''}`}>Speaker {row.index + 1}{!row.name && <small> (new)</small>}</span><input value={drafts[row.index] ?? row.name} onChange={(event) => setDrafts((current) => ({ ...current, [row.index]: event.target.value }))} onKeyDown={(event) => event.key === 'Enter' && void save(row.index)} placeholder="e.g. Maria Santos" aria-label={`Name for speaker ${row.index + 1}`} /><button className="secondary small" onClick={() => void save(row.index)}>Save</button></div>)}</div><div className="modal-actions"><button className="secondary" onClick={addSpeaker}>Add speaker</button><button className="primary" onClick={onClose}>Done</button></div></div></div>;
}
