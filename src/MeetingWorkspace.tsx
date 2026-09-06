import { useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { convertFileSrc } from '@tauri-apps/api/core';
import { save as saveDialog } from '@tauri-apps/plugin-dialog';
import { isSilenceSegment, speakerLabel } from './types';
import type { ContextEventRow, Meeting, Minutes, SpeakerName, TranscriptSegment } from './types';
import SpeakerEditor from './SpeakerEditor';
import ChatPanel from './ChatPanel';
import type { ChatMessage } from './ChatPanel';
import ClarificationsWizard from './ClarificationsWizard';
import type { ClarificationAnswer } from './ClarificationsWizard';
import { parseSlashCommand } from './chatActions';
import type { ClarificationQuestion } from './chatActions';
import Icon from './Icon';

type Props = { meeting: Meeting; onBack: () => void; onNotice: (message: string) => void; onImport: (meeting: Meeting) => void; onImportVtt: (meeting: Meeting) => void; onRecording: (meeting: Meeting, action: 'start' | 'pause' | 'resume' | 'stop') => void; onMeetingStatus: (meetingId: string, status: Meeting['status']) => void; onRetryTranscription: (meeting: Meeting) => void; busy?: boolean; transcribeProgress?: { completed: number; total: number } | null; liveSegments?: TranscriptSegment[] };
type Tab = 'overview' | 'transcript' | 'minutes' | 'evidence' | 'media' | 'chat';
const emptyMinutes = (title: string): Minutes => ({ title, summary: '', agenda: [], decisions: [], action_items: [], unresolved: [] });
const mmss = (seconds: number) => { const total = Math.max(0, Math.floor(seconds)); const hours = Math.floor(total / 3600); const minutes = Math.floor((total % 3600) / 60); const secs = total % 60; const two = (value: number) => value.toString().padStart(2, '0'); return hours > 0 ? `${hours}:${two(minutes)}:${two(secs)}` : `${two(minutes)}:${two(secs)}`; };

function Waveform({ progress = 0.28, peaks = [] }: { progress?: number; peaks?: number[] }) {
  return <div className="waveform" aria-label="Audio waveform"><div className="waveform-bars">{Array.from({ length: 64 }, (_, index) => { const peak = peaks[index] ?? (0.28 + (((index * 17) % 33) / 100)); return <span key={index} style={{ height: `${Math.max(12, Math.round(peak * 78))}%` }} />; })}</div><span className="waveform-progress" style={{ width: `${progress * 100}%` }} /></div>;
}

export default function MeetingWorkspace({ meeting, onBack, onNotice, onImport, onImportVtt, onRecording, onMeetingStatus, onRetryTranscription, busy, transcribeProgress, liveSegments = [] }: Props) {
  const [tab, setTab] = useState<Tab>('overview');
  const [minutes, setMinutes] = useState<Minutes>(() => emptyMinutes(meeting.title));
  const [transcript, setTranscript] = useState<TranscriptSegment[]>([]);
  const [query, setQuery] = useState('');
  const [playing, setPlaying] = useState(false);
  const [progress, setProgress] = useState(0.28);
  const [saving, setSaving] = useState(false);
  const [mediaCount, setMediaCount] = useState(0);
  const [peaks, setPeaks] = useState<number[]>([]);
  const [generating, setGenerating] = useState(false);
  const [audioUrl, setAudioUrl] = useState<string | null>(null);
  const [playbackSeconds, setPlaybackSeconds] = useState(0);
  const [mediaDuration, setMediaDuration] = useState(0);
  const [speakerNames, setSpeakerNames] = useState<SpeakerName[]>([]);
  const [segmentSpeakers, setSegmentSpeakers] = useState<Record<string, number[]>>({});
  const [showSpeakerEditor, setShowSpeakerEditor] = useState(false);
  const [overlapPicker, setOverlapPicker] = useState<string | null>(null);
  const [chatMessages, setChatMessages] = useState<ChatMessage[]>([]);
  const [chatBusy, setChatBusy] = useState(false);
  const [customFormat, setCustomFormat] = useState('');
  const [showCustomFormat, setShowCustomFormat] = useState(false);
  const [clarifyQuestions, setClarifyQuestions] = useState<ClarificationQuestion[] | null>(null);
  // Wall-clock seconds since the current transcription run started, so the
  // user can tell a healthy job from one that stopped making progress.
  const [elapsedSeconds, setElapsedSeconds] = useState(0);
  const audioRef = useRef<HTMLAudioElement | null>(null);

  useEffect(() => {
    if (!busy) { setElapsedSeconds(0); return; }
    const timer = window.setInterval(() => setElapsedSeconds((value) => value + 1), 1000);
    return () => window.clearInterval(timer);
  }, [busy]);

  useEffect(() => {
    let active = true;
    Promise.all([
      invoke<TranscriptSegment[]>('list_transcript_command', { meetingId: meeting.id }).catch(() => []),
      invoke<Minutes | null>('load_minutes_command', { meetingId: meeting.id }).catch(() => null),
      invoke<unknown[]>('list_media_command', { meetingId: meeting.id }).catch(() => []),
      invoke<[number, string][]>('list_speaker_names_command', { meetingId: meeting.id }).catch(() => []),
      invoke<Record<string, number[]>>('list_segment_speakers_command', { meetingId: meeting.id }).catch(() => ({})),
    ]).then(async ([segments, loadedMinutes, media, names, overlaps]) => { if (!active) return; setTranscript(segments); if (loadedMinutes) setMinutes(loadedMinutes); setMediaCount(media.length);
      setSpeakerNames((names as [number, string][]).map(([speaker_index, name]) => ({ speaker_index, name })));
      setSegmentSpeakers(overlaps as Record<string, number[]>);
      const playable = (media as Array<{ path?: string }>).find((item) => item.path && !item.path.toLowerCase().endsWith('.wav')) ?? (media as Array<{ path?: string }>).find((item) => item.path);
      if (playable?.path) setAudioUrl(convertFileSrc(playable.path));
      const wav = (media as Array<{ path?: string }>).find((item) => item.path?.toLowerCase().endsWith('.wav')); if (wav?.path) { const values = await invoke<number[]>('waveform_peaks_command', { path: wav.path, peakCount: 64 }).catch(() => []); if (active) setPeaks(values); } });
    return () => { active = false; };
  }, [meeting.id]);

  // A finished transcription run refetches the persisted transcript in App and
  // hands it over here, so the workspace never shows a gap between the live
  // stream ending and the stored segments arriving.
  useEffect(() => {
    const handleUpdated = (event: Event) => {
      const detail = (event as CustomEvent<{ meetingId: string; segments: TranscriptSegment[] }>).detail;
      if (detail?.meetingId !== meeting.id) return;
      // An empty segments payload signals "refetch from the database" (VTT
      // import dispatches it); a non-empty payload is the fresh segment list.
      if (detail.segments.length === 0) {
        invoke<TranscriptSegment[]>('list_transcript_command', { meetingId: meeting.id }).then((stored) => setTranscript(stored)).catch(() => undefined);
        return;
      }
      setTranscript(detail.segments);
    };
    window.addEventListener('bea:transcript-updated', handleUpdated);
    return () => window.removeEventListener('bea:transcript-updated', handleUpdated);
  }, [meeting.id]);

  // load chat history + custom minutes format once per meeting
  useEffect(() => {
    invoke<ContextEventRow[]>('list_context_events_command', { meetingId: meeting.id })
      .then((events) => setChatMessages(events.map((event) => ({ id: event.id, role: event.kind === 'chat' ? 'bea' : 'user', text: event.payload }))))
      .catch(() => undefined);
    invoke<string>('load_custom_minutes_format_command', { meetingId: meeting.id }).then(setCustomFormat).catch(() => undefined);
  }, [meeting.id]);

  async function sendChat(input: string) {
    const parsed = parseSlashCommand(input);
    const localId = crypto.randomUUID();
    if (parsed.action?.startsWith('unknown:')) {
      setChatMessages((current) => [...current, { id: localId, role: 'system', text: `Unknown action "${parsed.action!.split(':')[1]}". Try /clarify /context /correction /custom.` }]);
      return;
    }
    setChatMessages((current) => [...current, { id: localId, role: 'user', text: input.trim() }]);
    if (parsed.action === 'correction') {
      const [findText, replaceText] = parsed.arg.split('=>').map((part) => part.trim());
      if (!findText || replaceText === undefined) {
        setChatMessages((current) => [...current, { id: crypto.randomUUID(), role: 'system', text: 'Use /correction FIND=>REPLACE, e.g. /correction Hans=>Hansel.' }]);
        return;
      }
      try {
        const changed = await invoke<number>('apply_mass_correction_command', { meetingId: meeting.id, findText, replaceText });
        const stored = await invoke<TranscriptSegment[]>('list_transcript_command', { meetingId: meeting.id });
        setTranscript(stored);
        setChatMessages((current) => [...current, { id: crypto.randomUUID(), role: 'bea', text: `Replaced ${changed} transcript segment(s): “${findText}” → “${replaceText}”.` }]);
      } catch (error) { setChatMessages((current) => [...current, { id: crypto.randomUUID(), role: 'system', text: `Correction failed: ${String(error)}` }]); }
      return;
    }
    if (parsed.action === 'custom') { setShowCustomFormat(true); return; }
    if (parsed.action === 'clarify' || parsed.action === 'context') {
      if (!parsed.arg.trim()) { setChatMessages((current) => [...current, { id: crypto.randomUUID(), role: 'system', text: `Add a note after ${parsed.action}, e.g. “/${parsed.action} the vote passed 5-2”.` }]); return; }
      try {
        const event = await invoke<{ id: string }>('add_context_event_command', { meetingId: meeting.id, kind: parsed.action, payload: parsed.arg });
        setChatMessages((current) => [...current, { id: event.id, role: 'user', text: parsed.arg }]);
      } catch (error) { setChatMessages((current) => [...current, { id: crypto.randomUUID(), role: 'system', text: `Could not save: ${String(error)}` }]); }
      return;
    }
    // plain question → chat_command. A trailing `/vision 320, 480` requests
    // video frames at those seconds; it is stripped from the question text.
    setChatBusy(true);
    const visionMatch = input.trim().match(/\s*\/vision\s+([\d\s,]+)\s*$/i);
    const includeFrames = visionMatch
      ? visionMatch[1].split(',').map((part) => Number.parseInt(part.trim(), 10)).filter((value) => Number.isFinite(value) && value >= 0)
      : undefined;
    const question = visionMatch ? input.trim().slice(0, visionMatch.index).trim() : input.trim();
    try {
      const reply = await invoke<{ answer: string; frames_used: number; mode: string }>('chat_command', { meetingId: meeting.id, question, includeFrames });
      const chip = reply.mode && reply.frames_used > 0
        ? `📹 ${reply.frames_used} frame${reply.frames_used === 1 ? '' : 's'} (${reply.mode === 'vision' ? 'vision' : 'OCR fallback'})`
        : undefined;
      setChatMessages((current) => [...current, { id: crypto.randomUUID(), role: 'bea', text: reply.answer, chip }]);
    } catch (error) {
      setChatMessages((current) => [...current, { id: crypto.randomUUID(), role: 'system', text: `Chat failed: ${String(error)}` }]);
    } finally { setChatBusy(false); }
  }

  async function runMinutesGeneration() {
    setGenerating(true);
    try {
      const generated = await invoke<Minutes>('generate_minutes_command', { meetingId: meeting.id });
      setMinutes(generated);
      onNotice('Minutes generated from the timestamped transcript and your clarifications.');
    } catch (error) {
      onNotice(`Minutes could not be generated: ${String(error)}`);
    } finally { setGenerating(false); setClarifyQuestions(null); }
  }

  async function confirmClarifications(answers: ClarificationAnswer[]) {
    // Persist each answered question as a clarify context event so it feeds the
    // minutes prompt, the chat context, and survives restarts.
    for (const pair of answers) {
      try {
        await invoke('add_context_event_command', { meetingId: meeting.id, kind: 'clarify', payload: `${pair.question} → ${pair.answer}` });
      } catch { /* one failed note must not block generation */ }
    }
    setChatMessages((current) => [...current, ...answers.map((pair) => ({ id: crypto.randomUUID(), role: 'user' as const, text: pair.answer }))]);
    await runMinutesGeneration();
  }

  const mergedTranscript = useMemo(() => {
    const seen = new Set(transcript.map((segment) => segment.id));
    const pending = liveSegments.filter((segment) => !seen.has(segment.id));
    return [...transcript, ...pending].sort((a, b) => a.start_seconds - b.start_seconds);
  }, [transcript, liveSegments]);

  // Conversation view: consecutive segments from the same speaker merge into
  // one flowing block (per-speaker paragraphs) instead of one row per minute.
  // Silence placeholders and empty rows are dropped from the reading view —
  // their total is still summarized in the toolbar.
  type ConversationBlock = { key: string; speaker: number | null; startSeconds: number; endSeconds: number; text: string };
  const conversationBlocks = useMemo<ConversationBlock[]>(() => {
    const blocks: ConversationBlock[] = [];
    for (const segment of mergedTranscript) {
      if (isSilenceSegment(segment) || !segment.text.trim()) continue;
      const speaker = segment.speaker ?? null;
      const previous = blocks[blocks.length - 1];
      // Only merge adjacent segments if they have a known matching speaker (speaker !== null)
      // and are within a short pause. If speaker is null / unknown, keep segments distinct (or merge only within 5s).
      const sameKnownSpeaker = speaker !== null && previous && previous.speaker === speaker;
      const smallGap = previous ? segment.start_seconds - previous.endSeconds <= 5 : false;
      const canMerge = previous && (sameKnownSpeaker ? segment.start_seconds - previous.endSeconds <= 30 : smallGap && previous.speaker === null && previous.text.length < 80);
      if (canMerge) {
        previous.text = `${previous.text} ${segment.text.trim()}`;
        previous.endSeconds = segment.end_seconds;
      } else {
        blocks.push({ key: segment.id, speaker, startSeconds: segment.start_seconds, endSeconds: segment.end_seconds, text: segment.text.trim() });
      }
    }
    return blocks;
  }, [mergedTranscript]);

  const filteredBlocks = useMemo(() => conversationBlocks.filter((block) => !query.trim() || block.text.toLowerCase().includes(query.toLowerCase())), [conversationBlocks, query]);
  const filteredTranscript = filteredBlocks;
  const speakerCount = useMemo(() => new Set(mergedTranscript.map((segment) => segment.speaker).filter((speaker) => speaker !== null && speaker !== undefined)).size, [mergedTranscript]);
  const silenceSeconds = useMemo(() => mergedTranscript.reduce((total, segment) => isSilenceSegment(segment) ? total + Math.max(0, segment.end_seconds - segment.start_seconds) : total, 0), [mergedTranscript]);
  const evidence = useMemo(() => minutes.decisions.concat(minutes.action_items, minutes.unresolved).flatMap((item) => item.evidence), [minutes]);
  const status = meeting.status;

  async function save() { setSaving(true); try { await invoke('save_minutes_command', { meetingId: meeting.id, minutes }); onNotice('Minutes saved locally.'); } catch { onNotice('Saved in this session. Desktop persistence will be available when the app is running in Tauri.'); } finally { setSaving(false); } }
  async function generate() {
    if (!transcript.length) {
      onNotice('There is no transcript yet — record or import media first, then generate minutes from it.');
      return;
    }
    setGenerating(true);
    try {
      const suggestions = await invoke<{ question: string; options: string[] }[]>('suggest_clarifications_command', { meetingId: meeting.id })
        .catch(() => []); // offline or provider hiccup → generate without the wizard
      if (suggestions.length === 0) { await runMinutesGeneration(); return; }
      setClarifyQuestions(suggestions);
    } finally {
      setGenerating(false);
    }
  }
  async function exportNative(format: 'pdf' | 'docx') { try { await save(); const destination = await saveDialog({ defaultPath: `${minutes.title.replace(/[^a-z0-9]+/gi, '-').toLowerCase() || 'minutes'}.${format}`, filters: [{ name: format.toUpperCase(), extensions: [format] }] }); if (!destination) return; await invoke('export_minutes_command', { meetingId: meeting.id, format, destination }); onNotice(`${format.toUpperCase()} export saved.`); } catch (error) { onNotice(`Unable to export ${format.toUpperCase()}: ${String(error)}`); } }
  function downloadMarkdown() { const agenda = minutes.agenda.length ? `\n\n## Agenda\n${minutes.agenda.map((item) => { const span = item.start_seconds != null && item.end_seconds != null ? ` (${mmss(item.start_seconds)}\u2013${mmss(item.end_seconds)})` : ''; return `- ${item.heading}${span}`; }).join('\n')}` : ''; const text = `# ${minutes.title}\n\n${minutes.summary}${agenda}\n\n## Decisions\n${minutes.decisions.map((item) => `- ${item.summary}`).join('\n')}\n\n## Action items\n${minutes.action_items.map((item) => `- ${item.summary}`).join('\n')}\n\n## Unresolved\n${minutes.unresolved.map((item) => `- ${item.summary}`).join('\n')}`; const anchor = document.createElement('a'); anchor.href = URL.createObjectURL(new Blob([text], { type: 'text/markdown' })); anchor.download = `${minutes.title.replace(/[^a-z0-9]+/gi, '-').toLowerCase() || 'minutes'}.md`; anchor.click(); URL.revokeObjectURL(anchor.href); onNotice('Markdown export downloaded.'); }
  function updateSegment(id: string, text: string) { setTranscript((items) => items.map((item) => item.id === id ? { ...item, text } : item)); void invoke('update_transcript_segment_command', { segmentId: id, text }).catch(() => undefined); }
  async function assignSpeaker(segmentId: string, speakerIndex: number) {
    setTranscript((items) => items.map((item) => item.id === segmentId ? { ...item, speaker: speakerIndex } : item));
    try {
      await invoke('assign_segment_speaker_command', { segmentId, speakerIndex });
    } catch (error) {
      onNotice(`Could not assign the speaker: ${String(error)}`);
    }
  }
  async function applyOverlaps(segmentId: string, primary: number | null, selected: number[]) {
    // All indexes for the block, primary included; empty clears overlap rows.
    const all = primary !== null && !selected.includes(primary) ? [primary, ...selected] : selected;
    setSegmentSpeakers((current) => { const next = { ...current }; if (selected.length) next[segmentId] = all; else delete next[segmentId]; return next; });
    try {
      await invoke('set_segment_speakers_command', { segmentId, speakerIndexes: all });
    } catch (error) {
      onNotice(`Could not mark the overlap: ${String(error)}`);
    }
  }

  return <main className="workspace-shell"><aside className="workspace-rail"><button className="back-link" onClick={onBack}><Icon name="arrow-left" size={15} />Meetings</button><div className="workspace-rail-title"><span className="status-dot ready" />{meeting.title}</div><nav className="workspace-nav">{(['overview', 'transcript', 'minutes', 'evidence', 'media', 'chat'] as Tab[]).map((item) => <button key={item} className={tab === item ? 'active' : ''} onClick={() => setTab(item)}><Icon name={item === 'overview' ? 'grid' : item === 'transcript' ? 'waveform' : item === 'minutes' ? 'file' : item === 'evidence' ? 'image' : item === 'chat' ? 'spark' : 'folder'} size={15} />{item[0].toUpperCase() + item.slice(1)}{item === 'transcript' && mergedTranscript.length > 0 && <span className="nav-count">{mergedTranscript.length}</span>}</button>)}</nav><div className="workspace-rail-footer"><span className="local-dot" />Local project<small>{meeting.language === 'auto' ? 'Language auto-detect' : meeting.language}</small></div></aside><section className="workspace-main"><header className="workspace-header"><div><span className="workspace-breadcrumb">Meetings <Icon name="chevron-right" size={13} /> {tab[0].toUpperCase() + tab.slice(1)}</span><h1>{meeting.title}</h1></div><div className="workspace-header-actions"><span className={`status-label ${status} ${busy ? 'working' : ''}`}>{busy ? transcribeProgress ? `Transcribing… ${transcribeProgress.completed}/${transcribeProgress.total} · ${mmss(elapsedSeconds)} elapsed` : `Transcribing… ${mmss(elapsedSeconds)} elapsed` : status === 'recording' ? 'Recording' : status === 'processing' ? 'Processing — nothing is running' : status === 'ready' ? 'Ready' : statusLabel(status)}</span>{busy && <div className="workspace-progress" role="progressbar" aria-label="Transcription progress" aria-valuemin={0} aria-valuemax={100} aria-valuenow={transcribeProgress?.total ? Math.round((transcribeProgress.completed / transcribeProgress.total) * 100) : undefined}><span style={{ width: transcribeProgress?.total ? `${Math.round((transcribeProgress.completed / transcribeProgress.total) * 100)}%` : '100%' }} className={!transcribeProgress ? 'indeterminate' : undefined} /></div>}{status === 'processing' && !busy && <button className="secondary" onClick={() => onRetryTranscription(meeting)}><Icon name="refresh" size={15} />Retry transcription</button>}<button className="secondary" onClick={() => onImport(meeting)}><Icon name="upload" size={15} />Import</button>{status === 'recording' ? <button className="record-button active" onClick={() => onRecording(meeting, 'pause')}><Icon name="pause" size={15} />Pause</button> : status === 'paused' ? <button className="record-button" onClick={() => onRecording(meeting, 'resume')}><Icon name="mic" size={15} />Resume</button> : <button className="record-button" onClick={() => onRecording(meeting, 'start')}><Icon name="mic" size={15} />Record</button>}</div></header>{tab === 'overview' && <Overview transcript={transcript} minutes={minutes} meeting={meeting} onOpenTranscript={() => setTab('transcript')} onGenerate={() => void generate()} peaks={peaks} generating={generating} />}{tab === 'transcript' && <div className="workspace-grid"><section className="editor-column"><div className="player"><div className="player-heading"><div><span className="player-label">Recording</span><strong>{meeting.title}</strong></div><span>{mmss(Math.round(playbackSeconds))} / {mmss(mediaDuration || meeting.duration_seconds)}</span></div><Waveform progress={mediaDuration ? playbackSeconds / mediaDuration : progress} peaks={peaks} /><div className="player-controls"><button className="play-button" onClick={() => { const audio = audioRef.current; if (!audio) return; if (playing) { audio.pause(); } else { void audio.play(); } }}><Icon name={playing ? 'pause' : 'play'} size={15} /></button><input aria-label="Playback position" type="range" min="0" max="1" step="0.01" value={mediaDuration ? playbackSeconds / mediaDuration : 0} onChange={(event) => { const audio = audioRef.current; if (audio && mediaDuration) audio.currentTime = Number(event.target.value) * mediaDuration; }} /><span>1×</span></div>{audioUrl ? <audio ref={audioRef} src={audioUrl} preload="metadata" onPlay={() => setPlaying(true)} onPause={() => setPlaying(false)} onEnded={() => { setPlaying(false); setPlaybackSeconds(0); }} onTimeUpdate={(event) => setPlaybackSeconds(event.currentTarget.currentTime)} onLoadedMetadata={(event) => setMediaDuration(event.currentTarget.duration || 0)} /> : null}</div><div className="transcript-toolbar"><div><h2>Transcript</h2><span>{filteredTranscript.length} conversation turns{speakerCount > 0 ? ` · ${speakerCount} speaker${speakerCount === 1 ? '' : 's'} detected` : ''}{silenceSeconds > 0 ? ` · ${mmss(silenceSeconds)} silence` : ''}{busy ? ' · transcribing' : ''}</span></div><label className="search-field"><Icon name="search" size={14} /><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Find in transcript" /></label><button className="secondary small" onClick={() => onImportVtt(meeting)}><Icon name="file" size={14} />Import VTT</button><button className="secondary small" onClick={() => setShowSpeakerEditor(true)}><Icon name="grid" size={14} />Speakers</button></div><div className="transcript-editor">{filteredTranscript.length === 0 ? <div className="empty-editor"><Icon name="waveform" size={24} /><p>{query ? 'No transcript segment matches that search.' : 'No transcript yet. Record or import a file to begin.'}</p></div> : filteredTranscript.map((block) => { const extra = segmentSpeakers[block.key] ?? []; return <article className="conversation-block" key={block.key}><header className="conversation-header"><span className={`conversation-speaker${block.speaker !== null ? ' assigned' : ''}`}>{speakerLabel(block.speaker, speakerNames, extra)}</span><button className="segment-time" onClick={() => { const audio = audioRef.current; if (audio) { audio.currentTime = block.startSeconds; setPlaybackSeconds(block.startSeconds); } setProgress(meeting.duration_seconds ? block.startSeconds / meeting.duration_seconds : 0); }}>{mmss(block.startSeconds)}{block.endSeconds - block.startSeconds >= 60 ? `–${mmss(block.endSeconds)}` : ''}</button><span className="conversation-tools"><select className="speaker-select" value={block.speaker ?? ''} onChange={(event) => { const value = Number(event.target.value); if (!Number.isNaN(value)) void assignSpeaker(block.key, value); }} aria-label={`Speaker for ${mmss(block.startSeconds)}`}><option value="" disabled>Assign…</option>{speakerNames.map((entry) => <option key={entry.speaker_index} value={entry.speaker_index}>{entry.name}</option>)}</select><button className="overlap-button" onClick={() => setOverlapPicker(overlapPicker === block.key ? null : block.key)} aria-label={`Overlapping speakers for ${mmss(block.startSeconds)}`}>＋ overlap</button></span>{overlapPicker === block.key && <div className="overlap-picker">{speakerNames.filter((entry) => entry.speaker_index !== block.speaker).length === 0 ? <span className="inspector-muted">Name more speakers in the Speakers panel first.</span> : speakerNames.filter((entry) => entry.speaker_index !== block.speaker).map((entry) => { const checked = extra.includes(entry.speaker_index); return <label key={entry.speaker_index} className="overlap-option"><input type="checkbox" checked={checked} onChange={(event) => { const next = event.target.checked ? [...extra, entry.speaker_index] : extra.filter((i) => i !== entry.speaker_index); void applyOverlaps(block.key, block.speaker, next); }} />{entry.name}</label>; })}</div>}</header><textarea className="conversation-text" value={block.text} onChange={(event) => { const first = mergedTranscript.find((segment) => segment.id === block.key); if (first) updateSegment(block.key, event.target.value); }} rows={Math.max(2, Math.ceil(block.text.length / 110))} aria-label={`Transcript from ${mmss(block.startSeconds)}`} /></article>; })}</div></section><Inspector minutes={minutes} onGenerate={() => void generate()} generating={generating} hasTranscript={transcript.length > 0} /></div>}{tab === 'minutes' && <MinutesPanel minutes={minutes} setMinutes={setMinutes} saving={saving} onSave={() => void save()} onGenerate={() => void generate()} onMarkdown={downloadMarkdown} onExport={exportNative} generating={generating} hasTranscript={transcript.length > 0} />}{tab === 'evidence' && <EvidencePanel evidence={evidence} />}{tab === 'media' && <MediaPanel count={mediaCount} onImport={() => onImport(meeting)} />}{tab === 'chat' && <ChatPanel messages={chatMessages} busy={chatBusy} hasCustomFormat={customFormat.trim().length > 0} onSend={(input) => void sendChat(input)} onOpenCustomFormat={() => setShowCustomFormat(true)} />}</section>{showSpeakerEditor && <SpeakerEditor meetingId={meeting.id} names={speakerNames} onNamesChanged={setSpeakerNames} onNotice={onNotice} onClose={() => setShowSpeakerEditor(false)} />}{showCustomFormat && <div className="custom-format-overlay" role="dialog" aria-label="Custom minutes format"><div className="custom-format-modal"><div className="minutes-heading"><div><span className="page-kicker">Minutes format</span><h2>Custom minutes format</h2><p>Markdown the minutes must follow. Leave empty to use the default format.</p></div><button className="secondary small" onClick={() => setShowCustomFormat(false)}><Icon name="x" size={14} />Close</button></div><textarea className="custom-format-editor" value={customFormat} onChange={(event) => setCustomFormat(event.target.value)} rows={14} placeholder={'## Summary\n## Decisions\n## Actions — owner + due date\n## Open questions'} aria-label="Custom minutes format editor" /><div className="minutes-actions"><button className="primary" onClick={() => { void invoke('save_custom_minutes_format_command', { meetingId: meeting.id, format: customFormat }).then(() => onNotice('Custom minutes format saved.')).catch((error) => onNotice(`Could not save the format: ${String(error)}`)); setShowCustomFormat(false); }}><Icon name="check" size={14} />Save format</button><button className="secondary" onClick={() => { setCustomFormat(''); void invoke('save_custom_minutes_format_command', { meetingId: meeting.id, format: '' }).catch(() => undefined); }}>Reset to default</button></div></div></div>}{clarifyQuestions && <ClarificationsWizard questions={clarifyQuestions} busy={generating} onAnswered={(answers) => void confirmClarifications(answers)} onSkip={() => void runMinutesGeneration()} />}</main>;
}

function statusLabel(status: Meeting['status']) { return status === 'draft' ? 'Draft' : status === 'processing' ? 'Processing' : status === 'paused' ? 'Paused' : status === 'failed' ? 'Needs attention' : status[0].toUpperCase() + status.slice(1); }
function Overview({ transcript, minutes, meeting, onOpenTranscript, onGenerate, peaks, generating }: { transcript: TranscriptSegment[]; minutes: Minutes; meeting: Meeting; onOpenTranscript: () => void; onGenerate: () => void; peaks: number[]; generating: boolean }) { return <div className="overview-page"><div className="overview-hero"><div><span className="page-kicker">Meeting overview</span><h2>Keep the thread moving.</h2><p>{transcript.length ? 'Your transcript is ready to review. Bea keeps every decision connected to its source timestamp.' : 'Add a recording or import a file to create a timestamped transcript and meeting ledger. Minutes need a transcript first.'}</p><div className="overview-hero-actions"><button className="primary" onClick={onOpenTranscript}>{transcript.length ? 'Review transcript' : 'Open transcript'} <Icon name="arrow-right" size={14} /></button>{transcript.length > 0 && <button className="secondary" onClick={onGenerate} disabled={generating}><Icon name="spark" size={14} />{generating ? 'Generating…' : 'Generate minutes'}</button>}</div></div><div className="overview-wave"><Waveform progress={0.42} peaks={peaks} /><span>{meeting.duration_seconds ? `${Math.round(meeting.duration_seconds / 60)} min recorded` : 'No recording yet'}</span></div></div><div className="overview-cards"><div><span className="overview-card-icon"><Icon name="waveform" size={17} /></span><strong>{transcript.length}</strong><span>Transcript segments</span><button onClick={onOpenTranscript}>View transcript <Icon name="arrow-right" size={12} /></button></div><div><span className="overview-card-icon"><Icon name="spark" size={17} /></span><strong>{minutes.decisions.length}</strong><span>Decisions captured</span><button onClick={onGenerate} disabled={!transcript.length || generating}>Generate from transcript <Icon name="arrow-right" size={12} /></button></div><div><span className="overview-card-icon"><Icon name="check" size={17} /></span><strong>{minutes.action_items.length}</strong><span>Open action items</span><button onClick={onGenerate} disabled={!transcript.length || generating}>Build meeting ledger <Icon name="arrow-right" size={12} /></button></div></div></div> }
function Inspector({ minutes, onGenerate, generating, hasTranscript }: { minutes: Minutes; onGenerate: () => void; generating: boolean; hasTranscript: boolean }) { return <aside className="workspace-inspector"><div className="inspector-block"><div className="inspector-block-heading"><span>Summary</span><Icon name="more" size={15} /></div><p className="inspector-summary">{minutes.summary || 'Generate minutes after reviewing the transcript to see a concise meeting summary here.'}</p><button className="inspector-link" onClick={onGenerate} disabled={!hasTranscript || generating}><Icon name="spark" size={14} />{generating ? 'Generating…' : hasTranscript ? 'Generate minutes' : 'Add a transcript first'} <Icon name="arrow-right" size={13} /></button></div><div className="inspector-block"><div className="inspector-block-heading"><span>Decisions</span><span className="inspector-count">{minutes.decisions.length}</span></div>{minutes.decisions.length ? minutes.decisions.slice(0, 3).map((item) => <div className="inspector-item" key={item.summary}>{item.summary}</div>) : <p className="inspector-muted">No decisions extracted yet.</p>}</div><div className="inspector-block"><div className="inspector-block-heading"><span>Action items</span><span className="inspector-count">{minutes.action_items.length}</span></div>{minutes.action_items.length ? minutes.action_items.slice(0, 3).map((item) => <div className="inspector-item" key={item.summary}>{item.summary}</div>) : <p className="inspector-muted">No action items extracted yet.</p>}</div></aside> }
function MinutesPanel({ minutes, setMinutes, saving, onSave, onGenerate, onMarkdown, onExport, generating, hasTranscript }: { minutes: Minutes; setMinutes: (value: Minutes) => void; saving: boolean; onSave: () => void; onGenerate: () => void; onMarkdown: () => void; onExport: (format: 'pdf' | 'docx') => void; generating: boolean; hasTranscript: boolean }) { return <div className="minutes-page"><div className="minutes-heading"><div><span className="page-kicker">Reviewable output</span><h2>Meeting minutes</h2><p>{hasTranscript ? 'Keep the summary concise, then export the version you are ready to share.' : 'Minutes are generated from the transcript — record or import media first.'}</p></div><div className="minutes-actions"><button className="secondary" onClick={onGenerate} disabled={!hasTranscript || generating}><Icon name="spark" size={14} />{generating ? 'Generating…' : 'Generate'}</button><button className="primary" onClick={onSave} disabled={saving}>{saving ? 'Saving…' : 'Save minutes'}</button></div></div><div className="minutes-form"><label>Title<input value={minutes.title} onChange={(event) => setMinutes({ ...minutes, title: event.target.value })} /></label><label>Summary<textarea value={minutes.summary} onChange={(event) => setMinutes({ ...minutes, summary: event.target.value })} rows={7} placeholder="A concise summary will appear here after generation." /></label><div className="minutes-columns"><section className="minutes-agenda"><h3>Agenda <span>{minutes.agenda.length}</span></h3>{minutes.agenda.length ? minutes.agenda.map((item, idx) => <div className="ledger-item" key={idx}><input value={item.heading} onChange={(event) => setMinutes({ ...minutes, agenda: minutes.agenda.map((entry, i) => i === idx ? { ...entry, heading: event.target.value } : entry) })} aria-label={`Agenda item ${idx + 1}`} />{item.start_seconds != null && item.end_seconds != null && <small className="ledger-evidence">{mmss(item.start_seconds)}&ndash;{mmss(item.end_seconds)}</small>}<button className="secondary small" onClick={() => setMinutes({ ...minutes, agenda: minutes.agenda.filter((_, i) => i !== idx) })} aria-label={`Remove agenda item ${idx + 1}`}><Icon name="x" size={13} /></button></div>) : <div className="ledger-empty">Agenda items will be extracted with their timestamps.</div>}<button className="secondary small" onClick={() => setMinutes({ ...minutes, agenda: [...minutes.agenda, { heading: '', start_seconds: null, end_seconds: null }] })}><Icon name="plus" size={13} />Add agenda item</button></section><section><h3>Decisions <span>{minutes.decisions.length}</span></h3>{minutes.decisions.length ? minutes.decisions.map((item, idx) => <div className="ledger-item" key={idx}><Icon name="check" size={14} /><div><strong>{item.summary}</strong>{item.evidence?.[0] && <small className="ledger-evidence">{mmss(item.evidence[0].start_seconds)}: &ldquo;{item.evidence[0].quote}&rdquo;</small>}</div></div>) : <div className="ledger-empty">Decisions will be linked to their transcript timestamps.</div>}</section><section><h3>Action items <span>{minutes.action_items.length}</span></h3>{minutes.action_items.length ? minutes.action_items.map((item, idx) => <div className="ledger-item" key={idx}><Icon name="clock" size={14} /><div><strong>{item.summary}</strong>{item.evidence?.[0] && <small className="ledger-evidence">{mmss(item.evidence[0].start_seconds)}: &ldquo;{item.evidence[0].quote}&rdquo;</small>}</div></div>) : <div className="ledger-empty">Action items will be linked to their transcript timestamps.</div>}</section>{minutes.unresolved && minutes.unresolved.length > 0 && <section><h3>Open / Unresolved <span>{minutes.unresolved.length}</span></h3>{minutes.unresolved.map((item, idx) => <div className="ledger-item" key={idx}><Icon name="spark" size={14} /><div><strong>{item.summary}</strong>{item.evidence?.[0] && <small className="ledger-evidence">{mmss(item.evidence[0].start_seconds)}: &ldquo;{item.evidence[0].quote}&rdquo;</small>}</div></div>)}</section>}</div><div className="export-row"><button className="secondary" onClick={onMarkdown}>Export Markdown</button><button className="secondary" onClick={() => onExport('pdf')}>Export PDF</button><button className="secondary" onClick={() => onExport('docx')}>Export DOCX</button></div></div></div>; }
function EvidencePanel({ evidence }: { evidence: Array<{ start_seconds: number; end_seconds: number; quote: string }> }) { return <div className="evidence-page"><div className="minutes-heading"><div><span className="page-kicker">Source trail</span><h2>Evidence</h2><p>Every extracted item stays connected to the moment it came from.</p></div><span className="section-count">{evidence.length} linked moments</span></div>{evidence.length ? <div className="evidence-list">{evidence.map((item, index) => <article key={`${item.start_seconds}-${index}`}><button className="evidence-time"><Icon name="play" size={12} />{mmss(item.start_seconds)}</button><p>{item.quote}</p><Icon name="chevron-right" size={15} /></article>)}</div> : <div className="empty-editor"><Icon name="image" size={25} /><p>Evidence links will appear after Bea extracts decisions, actions, or unresolved questions.</p></div>}</div> }
function MediaPanel({ count, onImport }: { count: number; onImport: () => void }) { return <div className="media-page"><div className="minutes-heading"><div><span className="page-kicker">Project files</span><h2>Media</h2><p>Original audio and video stay local to this meeting project.</p></div><button className="primary" onClick={onImport}><Icon name="upload" size={14} />Import media</button></div><div className="media-empty"><span><Icon name="folder" size={24} /></span><strong>{count ? `${count} media source${count === 1 ? '' : 's'}` : 'No media added yet'}</strong><p>Drop a recording or choose a file to create the first source.</p><button className="secondary" onClick={onImport}>Choose a file</button></div></div> }
