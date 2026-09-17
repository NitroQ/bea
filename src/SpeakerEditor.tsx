import { useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { SpeakerName } from './types';
import Icon from './Icon';

type Props = {
  meetingId: string;
  names: SpeakerName[];
  detectedIndices: number[];
  onNamesChanged: (names: SpeakerName[]) => void;
  onNotice: (message: string) => void;
  onClose: () => void;
};

export default function SpeakerEditor({ meetingId, names, detectedIndices, onNamesChanged, onNotice, onClose }: Props) {
  const [drafts, setDrafts] = useState<Record<number, string>>({});
  const [added, setAdded] = useState<number[]>([]);
  const allKnownIndices = new Set([...names.map((n) => n.speaker_index), ...detectedIndices]);
  const addedCounter = useRef<number>(allKnownIndices.size ? Math.max(...allKnownIndices) + 1 : 0);

  function addSpeaker() {
    const index = Math.max(addedCounter.current, ...allKnownIndices, ...added, 0) + 1;
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
      onNotice(`Speaker ${index + 1} named "${name}".`);
    } catch (error) {
      onNotice(`Could not save the speaker name: ${String(error)}`);
    }
  }

  // Build the full list of rows: detected speakers (named or unnamed) + manually added speakers
  const namedIndices = new Set(names.map((n) => n.speaker_index));
  const pendingRows = added.filter((index) => !namedIndices.has(index));
  // Detected speakers that haven't been named yet
  const unnamedDetected = detectedIndices.filter((index) => !namedIndices.has(index) && !added.includes(index));

  const allRows = [
    ...names.map((entry) => ({ index: entry.speaker_index, name: entry.name, detected: detectedIndices.includes(entry.speaker_index) })),
    ...unnamedDetected.map((index) => ({ index, name: '', detected: true })),
    ...pendingRows.map((index) => ({ index, name: '', detected: false })),
  ].sort((a, b) => a.index - b.index);

  const hasAnyRows = allRows.length > 0;

  return <div className="modal-backdrop" role="presentation" onMouseDown={(event) => event.target === event.currentTarget && onClose()}><div className="modal compact-modal"><div className="modal-heading"><div><span className="page-kicker">Manual assignment</span><h2>Speakers</h2><p>{detectedIndices.length > 0 ? `${detectedIndices.length} speaker${detectedIndices.length === 1 ? '' : 's'} detected in the transcript. Name them below — all transcript segments with that speaker index will use the name you give.` : 'Name the people in this meeting, then assign transcript blocks to them from the transcript view.'}</p></div><button className="icon-button" onClick={onClose} aria-label="Close"><Icon name="x" size={17} /></button></div><div className="speaker-editor-rows">{!hasAnyRows && <p className="inspector-muted">No speakers detected — add the first one below.</p>}{allRows.map((row) => <div className="speaker-editor-row" key={row.index}><span className={`conversation-speaker${row.name ? ' assigned' : ''}`}>Speaker {row.index + 1}{!row.name && row.detected && <small> (detected)</small>}{!row.name && !row.detected && <small> (new)</small>}</span><input value={drafts[row.index] ?? row.name} onChange={(event) => setDrafts((current) => ({ ...current, [row.index]: event.target.value }))} onKeyDown={(event) => event.key === 'Enter' && void save(row.index)} placeholder="e.g. Maria Santos" aria-label={`Name for speaker ${row.index + 1}`} /><button className="secondary small" onClick={() => void save(row.index)}>Save</button></div>)}</div><div className="modal-actions"><button className="secondary" onClick={addSpeaker}>Add speaker</button><button className="primary" onClick={onClose}>Done</button></div></div></div>;
}
