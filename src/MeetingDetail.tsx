import { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { save as saveDialog } from '@tauri-apps/plugin-dialog';
import type { Meeting, Minutes, TranscriptSegment } from './types';

type Props = { meeting: Meeting; onClose: () => void; onNotice: (message: string) => void };
type Tab = 'overview' | 'minutes' | 'transcript' | 'evidence';

const emptyMinutes = (title: string): Minutes => ({ title, summary: '', agenda: [], decisions: [], action_items: [], unresolved: [] });

export default function MeetingDetail({ meeting, onClose, onNotice }: Props) {
  const [tab, setTab] = useState<Tab>('overview');
  const [minutes, setMinutes] = useState<Minutes>(() => emptyMinutes(meeting.title));
  const [transcript, setTranscript] = useState<TranscriptSegment[]>([]);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    invoke<TranscriptSegment[]>('list_transcript_command', { meetingId: meeting.id }).then(setTranscript).catch(() => setTranscript([]));
    invoke<Minutes | null>('load_minutes_command', { meetingId: meeting.id }).then((value) => value && setMinutes(value)).catch(() => undefined);
  }, [meeting.id]);

  const evidenceCount = useMemo(() => minutes.decisions.concat(minutes.action_items, minutes.unresolved).reduce((count, event) => count + event.evidence.length, 0), [minutes]);
  async function save() {
    setSaving(true);
    try { await invoke('save_minutes_command', { meetingId: meeting.id, minutes }); onNotice('Minutes saved locally.'); }
    catch { onNotice('Desktop persistence is unavailable in browser preview; edits remain visible in this session.'); }
    finally { setSaving(false); }
  }
  async function generate() {
    try {
      const generated = await invoke<Minutes>('generate_minutes_command', { meetingId: meeting.id });
      setMinutes(generated);
      onNotice('Minutes generated from the timestamped transcript and saved locally.');
    } catch (error) {
      onNotice(`Unable to generate minutes: ${String(error)}`);
    }
  }
  function downloadMarkdown() {
    const text = `# Minutes of the Meeting\n\n## ${minutes.title}\n\n${minutes.summary}`;
    const anchor = document.createElement('a'); anchor.href = URL.createObjectURL(new Blob([text], { type: 'text/markdown' })); anchor.download = `${minutes.title.replace(/[^a-z0-9]+/gi, '-').toLowerCase() || 'minutes'}.md`; anchor.click(); URL.revokeObjectURL(anchor.href); onNotice('Markdown export downloaded.');
  }

  async function exportNative(format: 'pdf' | 'docx') {
    try {
      await save();
      const destination = await saveDialog({
        defaultPath: `${minutes.title.replace(/[^a-z0-9]+/gi, '-').toLowerCase() || 'minutes'}.${format}`,
        filters: [{ name: format.toUpperCase(), extensions: [format] }],
      });
      if (!destination) return;
      await invoke('export_minutes_command', { meetingId: meeting.id, format, destination });
      onNotice(`${format.toUpperCase()} export saved.`);
    } catch (error) {
      onNotice(`Unable to export ${format.toUpperCase()}: ${String(error)}`);
    }
  }

  return <section className="detail panel"><div className="detail-heading"><div><p className="eyebrow">MEETING WORKSPACE</p><h2>{meeting.title}</h2><p>{meeting.language} · {Math.floor(meeting.duration_seconds / 60)}m recorded · {evidenceCount} evidence links</p></div><button className="secondary detail-close" onClick={onClose}>Back to library</button></div><nav className="tabs" aria-label="Meeting sections">{(['overview', 'minutes', 'transcript', 'evidence'] as Tab[]).map((item) => <button className={tab === item ? 'active' : ''} key={item} onClick={() => setTab(item)}>{item[0].toUpperCase() + item.slice(1)}</button>)}</nav>{tab === 'overview' && <div className="detail-copy"><h3>Meeting overview</h3><p>Raw audio and imported media remain local. Structured context is built from timestamped transcript evidence before any model request.</p><div className="overview-grid"><span><strong>{transcript.length}</strong> transcript segments</span><span><strong>{minutes.decisions.length}</strong> decisions</span><span><strong>{minutes.action_items.length}</strong> action items</span></div></div>}{tab === 'minutes' && <div className="minutes-editor"><label>Title<input value={minutes.title} onChange={(event) => setMinutes({ ...minutes, title: event.target.value })} /></label><label>Summary<textarea rows={6} value={minutes.summary} onChange={(event) => setMinutes({ ...minutes, summary: event.target.value })} placeholder="Review the generated summary before exporting." /></label><div className="editor-actions"><button onClick={generate}>Generate from transcript</button><button onClick={save} disabled={saving}>{saving ? 'Saving…' : 'Save minutes'}</button><button className="secondary" onClick={downloadMarkdown}>Export Markdown</button><button className="secondary" onClick={() => exportNative('pdf')}>Export PDF</button><button className="secondary" onClick={() => exportNative('docx')}>Export DOCX</button></div></div>}{tab === 'transcript' && <div className="transcript-list">{transcript.length === 0 ? <p className="muted">No transcript segments are available yet.</p> : transcript.map((segment) => <article key={segment.id}><time>{segment.start_seconds}s–{segment.end_seconds}s</time><p>{segment.text}</p></article>)}</div>}{tab === 'evidence' && <div className="transcript-list">{minutes.decisions.concat(minutes.action_items, minutes.unresolved).flatMap((event) => event.evidence).map((evidence, index) => <article key={`${evidence.start_seconds}-${index}`}><time>Evidence {evidence.start_seconds}s–{evidence.end_seconds}s</time><p>{evidence.quote}</p></article>)}{evidenceCount === 0 && <p className="muted">No extracted evidence links yet.</p>}</div>}</section>;
}
