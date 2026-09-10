import { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { AsrEngineId, Meeting, TranscriptLanguage } from './types';
import { statusLabel } from './types';
import Icon from './Icon';
import BeaAvatar from './BeaAvatar';
import Select from './Select';

type Props = {
  meetings: Meeting[];
  notice: string | null;
  onDismissNotice: () => void;
  selectedId: string | null;
  onSelect: (id: string) => void;
  onOpen: (meeting: Meeting) => void;
  onCreate: (title: string, language: TranscriptLanguage, engine: AsrEngineId) => Promise<void>;
  onImport: (meeting: Meeting) => void;
  onImportVtt: (meeting: Meeting) => void;
  onRename: (meeting: Meeting, title: string) => Promise<void>;
  onDelete: (meeting: Meeting) => Promise<void>;
  onSettings: () => void;
  runtimeReady: boolean;
  selectedEngineName: string;
};

function duration(seconds: number) { if (!seconds) return 'No recording yet'; const mins = Math.floor(seconds / 60); return `${mins}m ${String(seconds % 60).padStart(2, '0')}s`; }
function dateLabel(value: string) { const date = new Date(value); if (Number.isNaN(date.getTime())) return 'Just now'; return new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric', year: 'numeric' }).format(date); }

export default function Library({ meetings, notice, onDismissNotice, selectedId, onSelect, onOpen, onCreate, onImport, onImportVtt, onRename, onDelete, onSettings, runtimeReady, selectedEngineName }: Props) {
  const [query, setQuery] = useState('');
  const [sort, setSort] = useState<'recent' | 'name'>('recent');
  const [showNew, setShowNew] = useState(false);
  const [title, setTitle] = useState('');
  const [language, setLanguage] = useState<TranscriptLanguage>('auto');
  const [engine, setEngine] = useState<AsrEngineId>('whisper-compatibility');
  const [editing, setEditing] = useState<string | null>(null);
  const [renameValue, setRenameValue] = useState('');

  const visible = useMemo(() => meetings.filter((meeting) => meeting.title.toLowerCase().includes(query.toLowerCase())).sort((a, b) => sort === 'name' ? a.title.localeCompare(b.title) : new Date(b.created_at).getTime() - new Date(a.created_at).getTime()), [meetings, query, sort]);
  const selected = meetings.find((meeting) => meeting.id === selectedId) ?? visible[0] ?? null;
  const [hasMedia, setHasMedia] = useState(false);
  useEffect(() => {
    if (!selected) { setHasMedia(false); return; }
    invoke<unknown[]>('list_media_command', { meetingId: selected.id }).then((media) => setHasMedia(media.length > 0)).catch(() => setHasMedia(false));
  }, [selected?.id]);

  async function create() { if (!title.trim()) return; await onCreate(title.trim(), language, engine); setTitle(''); setShowNew(false); }
  async function rename(meeting: Meeting) { if (!renameValue.trim()) return; await onRename(meeting, renameValue.trim()); setEditing(null); }

  return <main className="product-shell">
    <aside className="product-rail"><div className="brand-lockup"><BeaAvatar variant="logo" size={36} /><div className="brand-mark compact"><span>bea</span><small>meeting assistant</small></div></div><nav className="product-nav"><button className="active"><Icon name="grid" size={16} />Meetings</button><button onClick={onSettings}><Icon name="settings" size={16} />Settings</button></nav><div className="rail-features">
          <div className="rail-features-heading"><Icon name="spark" size={13} />What Bea does</div>
          {[
            { icon: 'mic' as const, title: 'Record & transcribe', text: 'Local Whisper ASR, nothing uploaded' },
            { icon: 'file' as const, title: 'Import media or VTT', text: 'Audio, video, or Teams transcripts' },
            { icon: 'spark' as const, title: 'AI minutes', text: 'Summary, agenda, decisions, actions' },
            { icon: 'grid' as const, title: 'Ask the transcript', text: 'Chat with the meeting, cite timestamps' },
          ].map((feature) => (
            <div className="rail-feature" key={feature.title}>
              <span className="rail-feature-icon"><Icon name={feature.icon} size={13} /></span>
              <div><strong>{feature.title}</strong><small>{feature.text}</small></div>
            </div>
          ))}
        </div><div className="product-rail-footer"><BeaAvatar variant="happy" size={22} /><span>Bea workspace</span><Icon name="more" size={16} /></div></aside>
    <section className="library-main"><header className="library-header"><div><span className="page-kicker">Workspace</span><h1>Meetings</h1><p>Keep every recording, transcript, and decision in one place.</p></div><div className="library-header-actions"><button className="icon-button" aria-label="Settings" onClick={onSettings}><Icon name="settings" size={17} /></button><button className="primary" onClick={() => setShowNew(true)}><Icon name="plus" size={16} />New meeting</button></div></header><div className="library-toolbar"><label className="search-field"><Icon name="search" size={15} /><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Search meetings" /></label><div className="sort-field"><span>Sort</span><Select ariaLabel="Sort meetings" align="end" value={sort} onChange={setSort} options={[{ value: 'recent', label: 'Recently added' }, { value: 'name', label: 'Name' }]} /></div></div><div className="library-content"><div className="meeting-list"><button className="new-meeting-tile" onClick={() => setShowNew(true)}><span><Icon name="plus" size={22} /></span><div><strong>New meeting</strong><small>Record or import audio and video</small></div><Icon name="arrow-right" size={16} /></button>{visible.length === 0 ? <div className="library-empty"><BeaAvatar variant={query ? 'curious' : 'happy'} size={64} /><h2>{query ? 'No meetings found' : 'Your meeting library is empty'}</h2><p>{query ? 'Try a different search term.' : 'Create a meeting project to start collecting transcripts and decisions.'}</p></div> : visible.map((meeting) => <button key={meeting.id} className={`meeting-row ${selected?.id === meeting.id ? 'selected' : ''}`} onClick={() => onSelect(meeting.id)}><span className="meeting-row-icon"><Icon name={meeting.status === 'recording' ? 'mic' : meeting.status === 'ready' ? 'file' : 'clock'} size={16} /></span><span className="meeting-row-copy"><strong>{meeting.title}</strong><small>{dateLabel(meeting.created_at)} · {duration(meeting.duration_seconds)}</small></span><span className={`status-dot ${meeting.status}`} /><span className="meeting-row-status">{statusLabel[meeting.status]}</span><Icon name="chevron-right" size={15} /></button>)}</div><aside className="library-inspector">{selected ? <><div className="inspector-preview"><span className="preview-wave"><Icon name="waveform" size={28} /></span><span className="preview-duration">{duration(selected.duration_seconds)}</span></div><div className="inspector-title"><div><span className={`status-label ${selected.status}`}>{statusLabel[selected.status]}</span><h2>{selected.title}</h2><p>{selected.language === 'auto' ? 'Language detection' : selected.language} · {selectedEngineName}</p></div><button className="icon-button" onClick={() => { setEditing(selected.id); setRenameValue(selected.title); }} aria-label="Meeting actions"><Icon name="more" size={17} /></button></div><div className="inspector-actions"><button className="primary full" onClick={() => onOpen(selected)}>Open meeting <Icon name="arrow-right" size={15} /></button>{!hasMedia && <button className="secondary full" onClick={() => onImport(selected)}><Icon name="upload" size={15} />Import media</button>}<button className="secondary full" onClick={() => onImportVtt(selected)}><Icon name="file" size={15} />Import Teams VTT</button></div><div className="inspector-stats"><div><strong>—</strong><span>Transcript</span></div><div><strong>—</strong><span>Minutes</span></div><div><strong>{duration(selected.duration_seconds)}</strong><span>Duration</span></div></div><div className="inspector-footer"><span>Created {dateLabel(selected.created_at)}</span><button className="danger-button" onClick={() => void onDelete(selected)}><Icon name="trash" size={14} />Delete</button></div></> : <div className="inspector-empty"><Icon name="file" size={25} /><p>Select a meeting to see its project details.</p></div>}</aside></div></section>
    {showNew && <div className="modal-backdrop" role="presentation" onMouseDown={(event) => event.target === event.currentTarget && setShowNew(false)}><div className="modal"><div className="modal-heading"><div><span className="page-kicker">New project</span><h2>Create a meeting</h2><p>Start with a title. You can add media or record after opening it.</p></div><button className="icon-button" onClick={() => setShowNew(false)} aria-label="Close"><Icon name="x" size={17} /></button></div><label>Meeting title<input autoFocus value={title} onChange={(event) => setTitle(event.target.value)} placeholder="e.g. Product planning · August 14" onKeyDown={(event) => event.key === 'Enter' && void create()} /></label><div className="modal-field">Language<Select ariaLabel="Language" value={language} onChange={setLanguage} options={[{ value: 'auto', label: 'Auto-detect' }, { value: 'en', label: 'English' }, { value: 'fil', label: 'Filipino / Tagalog' }, { value: 'taglish', label: 'English + Filipino / Taglish' }]} /></div><div className="modal-field">Transcription engine<Select ariaLabel="Transcription engine" value={engine} onChange={setEngine} options={[{ value: 'whisper-compatibility', label: 'Whisper · recommended' }, { value: 'qwen-standard', label: 'Qwen Standard' }, { value: 'nemotron-multilingual', label: 'Nemotron multilingual' }]} /></div><div className="modal-actions"><button className="secondary" onClick={() => setShowNew(false)}>Cancel</button><button className="primary" onClick={() => void create()} disabled={!title.trim()}>Create meeting <Icon name="arrow-right" size={14} /></button></div></div></div>}
    {editing && selected && <div className="modal-backdrop" role="presentation" onMouseDown={(event) => event.target === event.currentTarget && setEditing(null)}><div className="modal compact-modal"><div className="modal-heading"><div><span className="page-kicker">Meeting project</span><h2>Rename meeting</h2></div><button className="icon-button" onClick={() => setEditing(null)} aria-label="Close"><Icon name="x" size={17} /></button></div><label>Title<input autoFocus value={renameValue} onChange={(event) => setRenameValue(event.target.value)} onKeyDown={(event) => event.key === 'Enter' && void rename(selected)} /></label><div className="modal-actions"><button className="secondary" onClick={() => setEditing(null)}>Cancel</button><button className="primary" onClick={() => void rename(selected)} disabled={!renameValue.trim()}>Save name</button></div></div></div>}
    {notice && <div className="workspace-toast" role="status" style={{ position: 'fixed', bottom: 24, right: 24, zIndex: 50 }}><Icon name="check" size={14} /><span>{notice}</span><button type="button" className="workspace-toast-close" onClick={onDismissNotice} aria-label="Dismiss notification"><Icon name="x" size={12} /></button></div>}
  </main>;
}
