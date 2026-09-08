import { useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { convertFileSrc } from '@tauri-apps/api/core';
import { save as saveDialog } from '@tauri-apps/plugin-dialog';
import { isSilenceSegment, speakerLabel } from './types';
import type { ContextEventRow, Meeting, Minutes, OpenRouterModelInfo, ProviderConfig, SpeakerName, TranscriptSegment } from './types';
import SpeakerEditor from './SpeakerEditor';
import ChatPanel from './ChatPanel';
import type { ChatMessage } from './ChatPanel';
import ClarificationsWizard from './ClarificationsWizard';
import BeaAvatar from './BeaAvatar';
import type { ClarificationAnswer } from './ClarificationsWizard';
import { parseSlashCommand } from './chatActions';
import type { ClarificationQuestion } from './chatActions';
import { modelLooksVisionCapable } from './ChatPanel';
import { CODEX_FALLBACK_MODELS } from './providerPresets';
import Icon from './Icon';
import type { IconName } from './Icon';

type AudioDeviceInfo = { id: string; name: string; is_default: boolean; sample_rate?: number | null; channels?: number | null; kind: string };
type Props = { meeting: Meeting; onBack: () => void; onNotice: (message: string) => void; onImport: (meeting: Meeting) => void; onImportVtt: (meeting: Meeting) => void; onRecording: (meeting: Meeting, action: 'start' | 'pause' | 'resume' | 'stop', deviceIds?: string[]) => void; onMeetingStatus: (meetingId: string, status: Meeting['status']) => void; onRetryTranscription: (meeting: Meeting) => void; busy?: boolean; transcribeProgress?: { completed: number; total: number } | null; liveSegments?: TranscriptSegment[] };
type Tab = 'overview' | 'transcript' | 'minutes' | 'evidence' | 'chat';
const emptyMinutes = (title: string): Minutes => ({ title, summary: '', agenda: [], visual_observations: [], decisions: [], action_items: [], unresolved: [] });
const mmss = (seconds: number) => { const total = Math.max(0, Math.floor(seconds)); const hours = Math.floor(total / 3600); const minutes = Math.floor((total % 3600) / 60); const secs = total % 60; const two = (value: number) => value.toString().padStart(2, '0'); return hours > 0 ? `${hours}:${two(minutes)}:${two(secs)}` : `${two(minutes)}:${two(secs)}`; };

const SPEEDS = [1, 1.25, 1.5, 2, 0.75];

export default function MeetingWorkspace({ meeting, onBack, onNotice, onImport, onImportVtt, onRecording, onMeetingStatus, onRetryTranscription, busy, transcribeProgress, liveSegments = [] }: Props) {
  const [tab, setTab] = useState<Tab>('overview');
  const [minutes, setMinutes] = useState<Minutes>(() => emptyMinutes(meeting.title));
  // In-workspace toast: App-level notices were invisible on the meeting
  // screen, which made generation failures look like silent no-ops.
  const [toast, setToast] = useState<{ message: string; error: boolean } | null>(null);
  const toastTimerRef = useRef<number | null>(null);
  const [transcript, setTranscript] = useState<TranscriptSegment[]>([]);
  const [query, setQuery] = useState('');
  const [playing, setPlaying] = useState(false);
  const [saving, setSaving] = useState(false);
  const [mediaCount, setMediaCount] = useState(0);
  const [generating, setGenerating] = useState(false);
  const [mediaUrl, setMediaUrl] = useState<string | null>(null);
  const [isVideo, setIsVideo] = useState(false);
  const [proxyNotice, setProxyNotice] = useState<string | null>(null);
  const proxyTriedRef = useRef(false);
  const [playbackSeconds, setPlaybackSeconds] = useState(0);
  const [mediaDuration, setMediaDuration] = useState(0);
  const [playbackRate, setPlaybackRate] = useState(1);
  const [volume, setVolume] = useState(1);
  const [muted, setMuted] = useState(false);
  const [speakerNames, setSpeakerNames] = useState<SpeakerName[]>([]);
  const [segmentSpeakers, setSegmentSpeakers] = useState<Record<string, number[]>>({});
  const [showSpeakerEditor, setShowSpeakerEditor] = useState(false);
  const [chatMessages, setChatMessages] = useState<ChatMessage[]>([]);
  const [chatBusy, setChatBusy] = useState(false);
  const [customFormat, setCustomFormat] = useState('');
  const [meetingModel, setMeetingModel] = useState('');
  const [meetingReasoning, setMeetingReasoning] = useState('');
  const [openRouterModels, setOpenRouterModels] = useState<OpenRouterModelInfo[]>([]);
  const [chatModelOptions, setChatModelOptions] = useState<string[]>([]);
  const [providerDefaultModel, setProviderDefaultModel] = useState('');
  useEffect(() => {
    let cancelled = false;
    invoke<ProviderConfig | null>('load_provider_command', { providerId: 'primary' }).then((saved) => {
      if (cancelled) return;
      setProviderDefaultModel(saved?.model ?? '');
      const kind = saved?.kind ?? 'OpenRouter';
      if (kind === 'OpenAiOAuth') {
        invoke<string[]>('codex_list_models_command').then((models) => { if (!cancelled) setChatModelOptions(models); }).catch(() => { if (!cancelled) setChatModelOptions(CODEX_FALLBACK_MODELS); });
      } else if (kind === 'OpenRouter') {
        invoke<OpenRouterModelInfo[]>('fetch_openrouter_models_command').then((models) => { if (!cancelled) { setOpenRouterModels(models); setChatModelOptions(models.map((m) => m.id)); } }).catch(() => { if (!cancelled) setChatModelOptions([]); });
      } else if (kind === 'Local' || kind === 'OpenAiCompatible') {
        invoke<OpenRouterModelInfo[]>('fetch_openrouter_models_command').then((models) => { if (!cancelled) setOpenRouterModels(models); }).catch(() => setOpenRouterModels([]));
        if (saved) {
          invoke<string[]>('discover_provider_models_command', { provider: saved, apiKey: '' }).then((ids) => {
            if (cancelled) return;
            if (ids.length) setChatModelOptions(ids);
            else invoke<OpenRouterModelInfo[]>('fetch_openrouter_models_command').then((m) => { if (!cancelled) setChatModelOptions(m.map((x) => x.id)); }).catch(() => {});
          }).catch(() => {
            invoke<OpenRouterModelInfo[]>('fetch_openrouter_models_command').then((m) => { if (!cancelled) setChatModelOptions(m.map((x) => x.id)); }).catch(() => {});
          });
        }
      } else {
        invoke<OpenRouterModelInfo[]>('fetch_openrouter_models_command').then((models) => { if (!cancelled) { setOpenRouterModels(models); setChatModelOptions(models.map((m) => m.id)); } }).catch(() => {});
      }
    }).catch(() => {
      invoke<OpenRouterModelInfo[]>('fetch_openrouter_models_command').then((models) => { if (!cancelled) { setOpenRouterModels(models); setChatModelOptions(models.map((m) => m.id)); } }).catch(() => {});
    });
    return () => { cancelled = true; };
  }, [meeting.id]);
  const [showCustomFormat, setShowCustomFormat] = useState(false);
  const [clarifyQuestions, setClarifyQuestions] = useState<ClarificationQuestion[] | null>(null);
  // Optional "include video frames" input removed — the AI pulls frames itself.
  const [minutesFrameInput, setMinutesFrameInput] = useState('');
  void minutesFrameInput; void setMinutesFrameInput;
  // Live recording telemetry: elapsed seconds (pause-aware) and input level
  // for the dedicated recording panel.
  const recordedBaseRef = useRef(0);
  const [recordingSeconds, setRecordingSeconds] = useState(0);
  const [recordingLevel, setRecordingLevel] = useState(0);
  const [audioDevices, setAudioDevices] = useState<AudioDeviceInfo[]>([]);
  const [selectedDevices, setSelectedDevices] = useState<string[]>([]);
  const recordingStatus = meeting.status;
  useEffect(() => {
    if (recordingStatus === 'recording') {
      const started = Date.now();
      const base = recordedBaseRef.current;
      const tick = window.setInterval(() => setRecordingSeconds(base + Math.floor((Date.now() - started) / 1000)), 500);
      const meter = window.setInterval(() => {
        invoke<number>('recording_level_command', { meetingId: meeting.id }).then(setRecordingLevel).catch(() => setRecordingLevel(0));
      }, 250);
      return () => {
        window.clearInterval(tick);
        window.clearInterval(meter);
        // Accumulate across pauses; the next run continues from here.
        recordedBaseRef.current = base + Math.floor((Date.now() - started) / 1000);
      };
    }
    if (recordingStatus !== 'paused') { recordedBaseRef.current = 0; setRecordingSeconds(0); }
    setRecordingLevel(0);
  }, [recordingStatus, meeting.id]);
  useEffect(() => {
    invoke<AudioDeviceInfo[]>('list_audio_input_devices_command').then((devices) => {
      setAudioDevices(devices);
      const defaultMic = devices.find((device) => device.kind === 'mic' && device.is_default);
      setSelectedDevices((current) => current.length ? current : defaultMic ? [defaultMic.id] : []);
    }).catch(() => setAudioDevices([]));
  }, []);
  const mediaRef = useRef<HTMLVideoElement | HTMLAudioElement | null>(null);
  // Wall-clock seconds since the current transcription run started, so the
  // user can tell a healthy job from one that stopped making progress.
  const [elapsedSeconds, setElapsedSeconds] = useState(0);
  useEffect(() => {
    if (!busy) { setElapsedSeconds(0); return; }
    const timer = window.setInterval(() => setElapsedSeconds((value) => value + 1), 1000);
    return () => window.clearInterval(timer);
  }, [busy]);
  const videoRef = mediaRef as React.RefObject<HTMLVideoElement | null>;
  const audioRef = mediaRef as React.RefObject<HTMLAudioElement | null>;
  // Keep rate/volume/mute in sync with the mounted media element (also covers
  // remounts when mediaUrl changes, e.g. after the playback proxy kicks in).
  useEffect(() => {
    const media = mediaRef.current;
    if (media) { media.playbackRate = playbackRate; media.volume = volume; media.muted = muted; }
  }, [playbackRate, volume, muted, mediaUrl]);

  useEffect(() => {
    if (!busy) { setElapsedSeconds(0); return; }
    const timer = window.setInterval(() => setElapsedSeconds((value) => value + 1), 1000);
    return () => window.clearInterval(timer);
  }, [busy]);

  // Refresh media count after transcription finishes or when media is imported
  useEffect(() => {
    if (busy) return;
    invoke<unknown[]>('list_media_command', { meetingId: meeting.id }).then((media) => setMediaCount(media.length)).catch(() => {});
    const handler = (event: Event) => {
      const detail = (event as CustomEvent<{ meetingId: string }>).detail;
      if (detail?.meetingId === meeting.id) {
        invoke<unknown[]>('list_media_command', { meetingId: meeting.id }).then((media) => setMediaCount(media.length)).catch(() => {});
      }
    };
    window.addEventListener('bea:media-updated', handler as EventListener);
    return () => window.removeEventListener('bea:media-updated', handler as EventListener);
  }, [busy, meeting.id]);

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
      const playable = (media as Array<{ path?: string; kind?: string }>).find((item) => item.path && !item.path.toLowerCase().endsWith('.wav')) ?? (media as Array<{ path?: string; kind?: string }>).find((item) => item.path);
      if (playable?.path) {
        proxyTriedRef.current = false;
        setIsVideo((playable.kind ?? 'video') !== 'audio');
        setMediaUrl(convertFileSrc(playable.path));
      }
      });
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
      // Chat log shows Q&A and user notes only — clarification answers collected
      // by the wizard stay out of the conversation view. A persisted 'chat'
      // event holds "Q: …\nA: …"; split it so the user's question shows as
      // their own bubble again instead of vanishing inside the answer.
      .then((events) => setChatMessages(events.filter((event) => event.kind !== 'clarify').flatMap((event) => {
        if (event.kind !== 'chat') return [{ id: event.id, role: 'user' as const, text: event.payload }];
        const pair = event.payload.match(/^Q: ([\s\S]*?)\nA: ([\s\S]*)$/);
        if (!pair) return [{ id: event.id, role: 'bea' as const, text: event.payload }];
        return [
          { id: `${event.id}-q`, role: 'user' as const, text: pair[1] },
          { id: `${event.id}-a`, role: 'bea' as const, text: pair[2] },
        ];
      })))
      .catch(() => undefined);
    invoke<string>('load_custom_minutes_format_command', { meetingId: meeting.id }).then(setCustomFormat).catch(() => undefined);
    invoke<string>('get_meeting_model_command', { meetingId: meeting.id }).then(setMeetingModel).catch(() => undefined);
    invoke<string>('get_meeting_reasoning_command', { meetingId: meeting.id }).then(setMeetingReasoning).catch(() => undefined);
  }, [meeting.id]);

  // App-level failures (transcription, import, recording) arrive here as
  // events so the meeting screen can show them in its own toast.
  useEffect(() => {
    const handler = (event: Event) => {
      const message = (event as CustomEvent<{ message?: string }>).detail?.message;
      if (message) notify(message);
    };
    window.addEventListener('bea:notice', handler);
    return () => window.removeEventListener('bea:notice', handler);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  function notify(message: string) {
    const error = /failed|could not|unable|error|needs attention/i.test(message);
    setToast({ message, error });
    if (toastTimerRef.current) window.clearTimeout(toastTimerRef.current);
    toastTimerRef.current = window.setTimeout(() => setToast(null), error ? 9000 : 5500);
    onNotice(message);
  }

  async function changeMeetingModel(model: string) {
    setMeetingModel(model);
    try { await invoke('set_meeting_model_command', { meetingId: meeting.id, model }); notify(model.trim() ? `This meeting now uses ${model.trim()}.` : 'Using the default model from Settings.'); } catch (error) { notify(`Could not save the model: ${String(error)}`); }
  }

  async function changeMeetingReasoning(reasoning: string) {
    setMeetingReasoning(reasoning);
    try { await invoke('set_meeting_reasoning_command', { meetingId: meeting.id, reasoning }); notify(reasoning.trim() ? `This meeting now uses ${reasoning.trim()} reasoning.` : 'Using the default reasoning effort from Settings.'); } catch (error) { notify(`Could not save the reasoning effort: ${String(error)}`); }
  }

  async function sendChat(input: string, images: string[] = []) {
    const parsed = parseSlashCommand(input);
    const localId = crypto.randomUUID();
    if (parsed.action?.startsWith('unknown:')) {
      setChatMessages((current) => [...current, { id: localId, role: 'system', text: `Unknown action "${parsed.action!.split(':')[1]}". Try /clarify /context /correction /custom.` }]);
      return;
    }
    setChatMessages((current) => [...current, { id: localId, role: 'user', text: input.trim() }]);
    if (parsed.action === 'correction') {
      // The instruction is natural language (e.g. "replace all "enyu" with
      // "NU""). The AI resolves it into concrete find/replace pairs against
      // the real transcript, then they are applied verbatim in SQL.
      setChatBusy(true);
      try {
        const result = await invoke<{ replacements: Array<{ find: string; replace: string }>; note: string }>('resolve_correction_command', { meetingId: meeting.id, instruction: parsed.arg });
        let total = 0;
        for (const pair of result.replacements) {
          total += await invoke<number>('apply_mass_correction_command', { meetingId: meeting.id, findText: pair.find, replaceText: pair.replace });
        }
        const stored = await invoke<TranscriptSegment[]>('list_transcript_command', { meetingId: meeting.id });
        setTranscript(stored);
        const summary = result.replacements.map((pair) => `“${pair.find}” → “${pair.replace}”`).join(', ');
        setChatMessages((current) => [...current, { id: crypto.randomUUID(), role: 'bea', text: total > 0 ? `Applied ${total} replacement(s): ${summary}` : `No matches found in the transcript. ${result.note}`.trim() }]);
      } catch (error) { setChatMessages((current) => [...current, { id: crypto.randomUUID(), role: 'system', text: `Correction failed: ${String(error)}` }]); }
      finally { setChatBusy(false); }
      return;
    }
    if (parsed.action === 'custom') { setShowCustomFormat(true); return; }
    if (parsed.action === 'minutes') {
      if (!parsed.arg.trim()) { setChatMessages((current) => [...current, { id: crypto.randomUUID(), role: 'system', text: 'Describe the change after /minutes, e.g. “/minutes add an agenda item about the budget review”.' }]); return; }
      setChatBusy(true);
      try {
        const updated = await invoke<Minutes>('modify_minutes_command', { meetingId: meeting.id, instruction: parsed.arg });
        setMinutes(updated);
        setChatMessages((current) => [...current, { id: crypto.randomUUID(), role: 'bea', text: 'Minutes updated. Check the Minutes tab for the changes.' }]);
      } catch (error) { setChatMessages((current) => [...current, { id: crypto.randomUUID(), role: 'system', text: `Could not modify minutes: ${String(error)}` }]); }
      finally { setChatBusy(false); }
      return;
    }
    if (parsed.action === 'clarify' || parsed.action === 'context') {
      if (!parsed.arg.trim()) { setChatMessages((current) => [...current, { id: crypto.randomUUID(), role: 'system', text: `Add a note after ${parsed.action}, e.g. “/${parsed.action} the vote passed 5-2”.` }]); return; }
      try {
        const event = await invoke<{ id: string }>('add_context_event_command', { meetingId: meeting.id, kind: parsed.action, payload: parsed.arg });
        setChatMessages((current) => [...current, { id: event.id, role: 'user', text: parsed.arg }]);
      } catch (error) { setChatMessages((current) => [...current, { id: crypto.randomUUID(), role: 'system', text: `Could not save: ${String(error)}` }]); }
      return;
    }
    // plain question → chat_command. The AI pulls video frames itself when the
    // meeting has a video; pasted/uploaded images ride along as attachments.
    setChatBusy(true);
    const question = input.trim();
    try {
      const reply = await invoke<{ answer: string; frames_used: number; mode: string }>('chat_command', { meetingId: meeting.id, question, images });
      const chip = reply.mode && reply.frames_used > 0
        ? `${reply.mode === 'vision' ? '📷' : '🔍'} ${reply.frames_used} image${reply.frames_used === 1 ? '' : 's'} (${reply.mode === 'vision' ? 'vision' : 'OCR fallback'})`
        : undefined;
      setChatMessages((current) => [...current, { id: crypto.randomUUID(), role: 'bea', text: reply.answer, chip }]);
    } catch (error) {
      setChatMessages((current) => [...current, { id: crypto.randomUUID(), role: 'system', text: `Chat failed: ${String(error)}` }]);
    } finally { setChatBusy(false); }
  }

  const generationRef = useRef<Promise<void> | null>(null);
  const cancelledRef = useRef(false);
  function startGeneration(): Promise<void> {
    // Single-flight: parallel callers (Generate + wizard refinement) share one
    // in-flight minutes call instead of racing two provider requests.
    if (generationRef.current) return generationRef.current;
    cancelledRef.current = false;
    const task = (async () => {
      try {
        const generated = await invoke<Minutes>('generate_minutes_command', { meetingId: meeting.id });
        // The user closed the wizard mid-run: drop the result instead of
        // showing minutes they explicitly stopped.
        if (cancelledRef.current) return;
        // Title fallback: when the AI returns a generic or empty title, use the meeting's own name.
        const generic = !generated.title.trim() || /^(meeting minutes|minutes)$/i.test(generated.title.trim());
        setMinutes(generic ? { ...generated, title: meeting.title } : generated);
        notify('Minutes generated from the timestamped transcript.');
      } catch (error) {
        if (cancelledRef.current || /cancelled:/i.test(String(error))) return;
        notify(`Minutes could not be generated: ${String(error)}`);
      } finally { setGenerating(false); }
    })();
    generationRef.current = task;
    void task.finally(() => { if (generationRef.current === task) generationRef.current = null; });
    return task;
  }

  async function generate() {
    if (!transcript.length) {
      notify('There is no transcript yet — record or import media first, then generate minutes from it.');
      return;
    }
    setGenerating(true);
    // Minutes generation starts immediately; clarification suggestions are
    // fetched in parallel. If the model finds real ambiguities the wizard
    // overlays while the first pass already runs — skipping it no longer
    // doubles the wait.
    const generation = startGeneration();
    const suggestions = await invoke<{ question: string; options: string[] }[]>('suggest_clarifications_command', { meetingId: meeting.id })
      .catch(() => []); // offline or provider hiccup → generate without the wizard
    if (cancelledRef.current) return; // the wizard was closed mid-flight
    if (suggestions.length > 0) {
      setClarifyQuestions(suggestions);
    }
    await generation;
  }

  /// The wizard's Close: stop minutes generation since nothing was answered.
  function closeClarifications() {
    setClarifyQuestions(null);
    if (generationRef.current) {
      cancelledRef.current = true;
      void invoke('cancel_minutes_generation_command', { meetingId: meeting.id }).catch(() => undefined);
      setGenerating(false);
      notify('Minutes generation stopped — you closed the questions without submitting.');
    }
  }

  async function confirmClarifications(answers: ClarificationAnswer[]) {
    setClarifyQuestions(null);
    // Persist each answered question as a clarify context event so it feeds the
    // minutes prompt and survives restarts. Answers stay out of the chat log —
    // the wizard was the UI for them.
    for (const pair of answers) {
      try {
        await invoke('add_context_event_command', { meetingId: meeting.id, kind: 'clarify', payload: `${pair.question} → ${pair.answer}` });
      } catch { /* one failed note must not block generation */ }
    }
    // The first pass may still be running (it starts in parallel with the
    // wizard). Wait for it, THEN fold the answers in — queuing the refinement
    // behind the in-flight call instead of silently reusing its answer-less
    // result.
    if (!answers.length) return;
    setGenerating(true);
    if (generationRef.current) {
      try { await generationRef.current; } catch { /* its error already surfaced; the refinement below still runs */ }
    }
    await startGeneration();
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

  const inDesktop = Boolean((window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__);
  async function save() { setSaving(true); try { await invoke('save_minutes_command', { meetingId: meeting.id, minutes }); notify('Minutes saved locally.'); } catch (error) { const message = String(error); if (!inDesktop && /not found|unknown command|tauri/i.test(message)) { notify('Saved in this session. Desktop persistence will be available when the app is running in Tauri.'); } else { notify(`Could not save minutes: ${message}`); } } finally { setSaving(false); } }
  async function exportMinutes(format: 'markdown' | 'pdf' | 'docx') {
    try {
      await save();
      if (format === 'markdown') { downloadMarkdown(); return; }
      const extension = format === 'pdf' ? 'pdf' : 'docx';
      const destination = await saveDialog({ defaultPath: `${minutes.title.replace(/[^a-z0-9]+/gi, '-').toLowerCase() || 'minutes'}.${extension}`, filters: [{ name: extension.toUpperCase(), extensions: [extension] }] });
      if (!destination) return;
      await invoke('export_minutes_command', { meetingId: meeting.id, format: extension, destination });
      notify(`${extension.toUpperCase()} export saved.`);
    } catch (error) { notify(`Unable to export: ${String(error)}`); }
  }
  function downloadMarkdown() { const agenda = minutes.agenda.length ? `\n\n## Agenda\n${minutes.agenda.map((item) => { const span = item.start_seconds != null && item.end_seconds != null ? ` (${mmss(item.start_seconds)}\u2013${mmss(item.end_seconds)})` : ''; return `- ${item.heading}${span}`; }).join('\n')}` : ''; const text = `# ${minutes.title}\n\n${minutes.summary}${agenda}\n\n## Decisions\n${minutes.decisions.map((item) => `- ${item.summary}`).join('\n')}\n\n## Action items\n${minutes.action_items.map((item) => `- ${item.summary}`).join('\n')}\n\n## Unresolved\n${minutes.unresolved.map((item) => `- ${item.summary}`).join('\n')}`; const anchor = document.createElement('a'); anchor.href = URL.createObjectURL(new Blob([text], { type: 'text/markdown' })); anchor.download = `${minutes.title.replace(/[^a-z0-9]+/gi, '-').toLowerCase() || 'minutes'}.md`; anchor.click(); URL.revokeObjectURL(anchor.href); notify('Markdown export downloaded.'); }
  function updateSegment(id: string, text: string) { setTranscript((items) => items.map((item) => item.id === id ? { ...item, text } : item)); void invoke('update_transcript_segment_command', { segmentId: id, text }).catch((error) => { notify(`Edit may not be saved: ${String(error)}`); }); }
  /// Speaker-picker apply: selection[0] is the primary speaker, the rest are
  /// overlapping speakers. An empty selection clears the block entirely.
  async function applySpeakerSelection(segmentId: string, selection: number[]) {
    const primary = selection.length ? selection[0] : null;
    setTranscript((items) => items.map((item) => item.id === segmentId ? { ...item, speaker: primary } : item));
    setSegmentSpeakers((current) => { const next = { ...current }; if (selection.length > 1) next[segmentId] = selection; else delete next[segmentId]; return next; });
    try {
      await invoke('set_segment_speakers_command', { segmentId, speakerIndexes: selection });
    } catch (error) {
      notify(`Could not update the speakers: ${String(error)}`);
    }
  }

  const effectiveChatModel = meetingModel || providerDefaultModel;
  const speedLabel = `${playbackRate}×`;
  function skip(delta: number) { const media = mediaRef.current; if (!media) return; media.currentTime = Math.max(0, media.currentTime + delta); setPlaybackSeconds(media.currentTime); }
  const chatVisionCapable = effectiveChatModel ? modelLooksVisionCapable(effectiveChatModel, openRouterModels) : null;

  return <main className="workspace-shell"><aside className="workspace-rail"><button type="button" className="rail-back" onClick={onBack} aria-label="Back to the meeting list" title="Back to meetings"><Icon name="arrow-left" size={15} />All meetings</button><nav className="workspace-nav">{(['overview', 'transcript', 'minutes', 'evidence', 'chat'] as Tab[]).map((item) => <button key={item} className={tab === item ? 'active' : ''} onClick={() => setTab(item)}><Icon name={item === 'overview' ? 'grid' : item === 'transcript' ? 'waveform' : item === 'minutes' ? 'file' : item === 'evidence' ? 'image' : 'spark'} size={15} />{item[0].toUpperCase() + item.slice(1)}{item === 'transcript' && mergedTranscript.length > 0 && <span className="nav-count">{mergedTranscript.length}</span>}</button>)}</nav><div className="workspace-rail-footer"><span className="local-dot" />Local project<small>{meeting.language === 'auto' ? 'Language auto-detect' : meeting.language}</small></div></aside><section className="workspace-main"><header className="workspace-header"><div className="workspace-title-lockup"><BeaAvatar variant="presenting" size={46} /><div><span className="workspace-breadcrumb">{tab[0].toUpperCase() + tab.slice(1)}</span><h1>{meeting.title}</h1></div></div><div className="workspace-header-actions"><span className={`status-label ${status} ${busy ? 'working' : ''}`}>{busy ? transcribeProgress ? `Transcribing… ${transcribeProgress.completed}/${transcribeProgress.total} · ${mmss(elapsedSeconds)} elapsed` : `Transcribing… ${mmss(elapsedSeconds)} elapsed` : status === 'recording' ? 'Recording' : status === 'processing' ? 'Processing — nothing is running' : status === 'ready' ? 'Ready' : statusLabel(status)}</span>{busy && <div className="workspace-progress" role="progressbar" aria-label="Transcription progress" aria-valuemin={0} aria-valuemax={100} aria-valuenow={transcribeProgress?.total ? Math.round((transcribeProgress.completed / transcribeProgress.total) * 100) : undefined}><span style={{ width: transcribeProgress?.total ? `${Math.round((transcribeProgress.completed / transcribeProgress.total) * 100)}%` : '100%' }} className={!transcribeProgress ? 'indeterminate' : undefined} /></div>}{status === 'processing' && !busy && <button className="secondary" onClick={() => onRetryTranscription(meeting)}><Icon name="refresh" size={15} />Retry transcription</button>}{mediaCount === 0 && <button className="secondary" onClick={() => onImport(meeting)}><Icon name="upload" size={15} />Import</button>}{(status === 'recording' || status === 'paused') ? <RecordingBar recording={status === 'recording'} seconds={recordingSeconds} level={recordingLevel} onPauseResume={() => onRecording(meeting, status === 'recording' ? 'pause' : 'resume')} onStop={() => onRecording(meeting, 'stop')} /> : mediaCount === 0 && <RecordLauncher devices={audioDevices} selected={selectedDevices} onToggle={(id) => setSelectedDevices((current) => current.includes(id) ? current.filter((entry) => entry !== id) : [...current, id])} onStart={() => onRecording(meeting, 'start', selectedDevices)} disabled={generating || busy} />}</div></header>{tab === 'overview' && <Overview transcript={transcript} minutes={minutes} meeting={meeting} onOpenTranscript={() => setTab('transcript')} onGenerate={() => void generate()} generating={generating} />}{tab === 'transcript' && <div className="workspace-grid"><section className="editor-column"><div className="player"><div className="player-heading"><div><span className="player-label">Recording</span><strong>{meeting.title}</strong></div><span>{mmss(Math.round(playbackSeconds))} / {mmss(mediaDuration || meeting.duration_seconds)}</span></div><div className="player-timeline"><input aria-label="Playback position" type="range" min="0" max="1" step="0.01" value={mediaDuration ? playbackSeconds / mediaDuration : 0} onChange={(event) => { const media = mediaRef.current; if (media && mediaDuration) { media.currentTime = Number(event.target.value) * mediaDuration; setPlaybackSeconds(media.currentTime); } }} /></div><div className="player-controls"><div className="player-volume"><button className="player-pill" aria-label={muted || volume === 0 ? 'Unmute' : 'Mute'} onClick={() => { if (muted || volume === 0) { setMuted(false); if (volume === 0) setVolume(0.6); } else setMuted(true); }}><Icon name={muted || volume === 0 ? 'volume-mute' : 'volume'} size={15} /></button><input aria-label="Volume" type="range" min="0" max="1" step="0.05" value={muted ? 0 : volume} onChange={(event) => { const next = Number(event.target.value); setVolume(next); setMuted(next === 0); }} /></div><div className="player-center"><button className="player-skip" aria-label="Back 10 seconds" onClick={() => skip(-10)}><Icon name="rewind-10" size={20} /></button><button className="play-button" aria-label={playing ? 'Pause' : 'Play'} onClick={() => { const media = mediaRef.current; if (!media) return; if (playing) { media.pause(); } else { void media.play(); } }}><Icon name={playing ? 'pause' : 'play'} size={16} /></button><button className="player-skip" aria-label="Forward 10 seconds" onClick={() => skip(10)}><Icon name="forward-10" size={20} /></button></div><div className="player-speed"><button className="player-pill" aria-label="Playback speed" onClick={() => setPlaybackRate((rate) => SPEEDS[(SPEEDS.indexOf(rate) + 1) % SPEEDS.length] ?? 1)}>{speedLabel}</button></div></div>{proxyNotice && <small className="proxy-notice">{proxyNotice}</small>}{mediaUrl && isVideo ? <video ref={videoRef} src={mediaUrl} preload="metadata" playsInline onPlay={() => setPlaying(true)} onPause={() => setPlaying(false)} onEnded={() => { setPlaying(false); setPlaybackSeconds(0); }} onTimeUpdate={(event) => setPlaybackSeconds(event.currentTarget.currentTime)} onLoadedMetadata={(event) => setMediaDuration(event.currentTarget.duration || 0)} onError={() => { if (proxyTriedRef.current) return; proxyTriedRef.current = true; setProxyNotice('Preparing playback…'); invoke<{ path: string }>('ensure_playable_proxy_command', { meetingId: meeting.id, mediaPath: decodeURIComponent(new URL(mediaUrl).pathname.replace(/^\/[A-Za-z]:/, '')) }).then((proxy) => { setMediaUrl(convertFileSrc(proxy.path)); setProxyNotice(null); }).catch((error) => { setProxyNotice(null); notify(`Playback failed: ${String(error)}`); }); }} style={{ width: '100%', borderRadius: 8, background: '#000' }} /> : null}{mediaUrl && !isVideo ? <audio ref={audioRef} src={mediaUrl} preload="metadata" onPlay={() => setPlaying(true)} onPause={() => setPlaying(false)} onEnded={() => { setPlaying(false); setPlaybackSeconds(0); }} onTimeUpdate={(event) => setPlaybackSeconds(event.currentTarget.currentTime)} onLoadedMetadata={(event) => setMediaDuration(event.currentTarget.duration || 0)} /> : null}</div><div className="transcript-toolbar"><div><h2>Transcript</h2><span>{filteredTranscript.length} conversation turns{speakerCount > 0 ? ` · ${speakerCount} speaker${speakerCount === 1 ? '' : 's'} detected` : ''}{silenceSeconds > 0 ? ` · ${mmss(silenceSeconds)} silence` : ''}{busy ? ' · transcribing' : ''}</span></div><label className="search-field"><Icon name="search" size={14} /><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Find in transcript" /></label><button className="secondary small" onClick={() => onImportVtt(meeting)}><Icon name="file" size={14} />Import VTT</button><button className="secondary small" onClick={() => setShowSpeakerEditor(true)}><Icon name="grid" size={14} />Speakers</button></div><div className="transcript-editor">{filteredTranscript.length === 0 ? <div className="empty-editor"><Icon name="waveform" size={24} /><p>{query ? 'No transcript segment matches that search.' : 'No transcript yet. Record or import a file to begin.'}</p></div> : filteredTranscript.map((block) => { const extra = segmentSpeakers[block.key] ?? []; return <article className="conversation-block" key={block.key}><header className="conversation-header"><span className={`conversation-speaker${block.speaker !== null ? ' assigned' : ''}`}>{speakerLabel(block.speaker, speakerNames, extra)}</span><button className="segment-time" onClick={() => { const media = mediaRef.current; if (media) { media.currentTime = block.startSeconds; setPlaybackSeconds(block.startSeconds); } }}>{mmss(block.startSeconds)}{block.endSeconds - block.startSeconds >= 60 ? `–${mmss(block.endSeconds)}` : ''}</button><span className="conversation-tools"><SpeakerPicker primary={block.speaker} extras={extra} names={speakerNames} onApply={(selection) => void applySpeakerSelection(block.key, selection)} /></span></header><textarea className="conversation-text" value={block.text} onChange={(event) => { const first = mergedTranscript.find((segment) => segment.id === block.key); if (first) updateSegment(block.key, event.target.value); }} rows={Math.max(2, Math.ceil(block.text.length / 110))} aria-label={`Transcript from ${mmss(block.startSeconds)}`} /></article>; })}</div></section><Inspector minutes={minutes} onGenerate={() => void generate()} generating={generating} hasTranscript={transcript.length > 0} /></div>}{tab === 'minutes' && <MinutesPanel minutes={minutes} onGenerate={() => void generate()} onExport={(format) => void exportMinutes(format)} generating={generating} hasTranscript={transcript.length > 0} />}{tab === 'evidence' && <EvidencePanel evidence={evidence} />}{tab === 'chat' && <ChatPanel messages={chatMessages} busy={chatBusy} hasCustomFormat={customFormat.trim().length > 0} meetingModel={meetingModel} modelOptions={chatModelOptions.length ? chatModelOptions : openRouterModels.map((model) => model.id)} openRouterModels={openRouterModels} meetingReasoning={meetingReasoning} onReasoningChange={(reasoning) => void changeMeetingReasoning(reasoning)} visionCapable={chatVisionCapable} onModelChange={(model) => void changeMeetingModel(model)} onSend={(input, images) => void sendChat(input, images)} onOpenCustomFormat={() => setShowCustomFormat(true)} />}</section>{showSpeakerEditor && <SpeakerEditor meetingId={meeting.id} names={speakerNames} onNamesChanged={setSpeakerNames} onNotice={onNotice} onClose={() => setShowSpeakerEditor(false)} />}{showCustomFormat && <div className="custom-format-overlay" role="dialog" aria-label="Custom minutes format"><div className="custom-format-modal"><div className="minutes-heading"><div><span className="page-kicker">Minutes format</span><h2>Custom minutes format</h2><p>Markdown the minutes must follow. Leave empty to use the default format.</p></div><button className="secondary small" onClick={() => setShowCustomFormat(false)}><Icon name="x" size={14} />Close</button></div><textarea className="custom-format-editor" value={customFormat} onChange={(event) => setCustomFormat(event.target.value)} rows={14} placeholder={'## Summary\n## Decisions\n## Actions — owner + due date\n## Open questions'} aria-label="Custom minutes format editor" /><div className="minutes-actions"><button className="primary" onClick={() => { void invoke('save_custom_minutes_format_command', { meetingId: meeting.id, format: customFormat }).then(() => notify('Custom minutes format saved.')).catch((error) => notify(`Could not save the format: ${String(error)}`)); setShowCustomFormat(false); }}><Icon name="check" size={14} />Save format</button><button className="secondary" onClick={() => { setCustomFormat(''); void invoke('save_custom_minutes_format_command', { meetingId: meeting.id, format: '' }).catch(() => undefined); }}>Reset to default</button></div></div></div>}{clarifyQuestions && <ClarificationsWizard questions={clarifyQuestions} busy={generating} onAnswered={(answers) => void confirmClarifications(answers)} onClose={closeClarifications} />}{toast && <div className={`workspace-toast ${toast.error ? 'error' : ''}`} role="status"><Icon name={toast.error ? 'x' : 'check'} size={14} /><span>{toast.message}</span><button type="button" className="workspace-toast-close" onClick={() => setToast(null)} aria-label="Dismiss notification"><Icon name="x" size={12} /></button></div>}</main>;
}

function statusLabel(status: Meeting['status']) { return status === 'draft' ? 'Draft' : status === 'processing' ? 'Processing' : status === 'paused' ? 'Paused' : status === 'failed' ? 'Needs attention' : status[0].toUpperCase() + status.slice(1); }

function LevelMeter({ level }: { level: number }) { const bars = 12; const active = Math.round(Math.min(1, Math.max(0, level)) * bars); return <span className="level-meter" aria-label="Live input level">{Array.from({ length: bars }, (_, index) => <span key={index} className={index < active ? `bar bar-${index < 4 ? 'low' : index < 9 ? 'mid' : 'high'} on` : 'bar'} />)}</span>; }

function RecordingBar({ recording, seconds, level, onPauseResume, onStop }: { recording: boolean; seconds: number; level: number; onPauseResume: () => void; onStop: () => void }) {
  return <div className={`recording-bar ${recording ? 'live' : 'paused'}`}>
    <span className={`record-dot ${recording ? 'live' : ''}`} aria-hidden="true" />
    <span className="record-time" aria-label="Recording elapsed time">{recording ? 'REC' : 'PAUSED'} {mmss(seconds)}</span>
    <LevelMeter level={level} />
    <button className="secondary small" onClick={onPauseResume}><Icon name={recording ? 'pause' : 'play'} size={13} />{recording ? 'Pause' : 'Resume'}</button>
    <button className="danger-button small" onClick={onStop}><Icon name="check" size={13} />Stop &amp; transcribe</button>
  </div>;
}

function RecordLauncher({ devices, selected, onToggle, onStart, disabled }: { devices: AudioDeviceInfo[]; selected: string[]; onToggle: (id: string) => void; onStart: () => void; disabled?: boolean }) {
  const microphones = devices.filter((device) => device.kind === 'mic');
  const outputs = devices.filter((device) => device.kind === 'output');
  return <div className="record-launcher">
    <details className="record-sources">
      <summary><Icon name="mic" size={14} />Sources{selected.length > 0 && <span className="nav-count">{selected.length}</span>}</summary>
      <div className="record-sources-list">
        <div className="record-source-group">
          <strong>Microphone</strong>
          {microphones.length === 0 && <small>No microphone found</small>}
          {microphones.map((device) => <label key={device.id} className="record-source"><input type="checkbox" checked={selected.includes(device.id)} onChange={() => onToggle(device.id)} /><span>{device.name}{device.is_default ? ' (default)' : ''}</span><small>{device.sample_rate ? `${Math.round(device.sample_rate / 1000)} kHz` : ''}</small></label>)}
        </div>
        <div className="record-source-group">
          <strong>Desktop sound</strong>
          <small className="record-source-hint">Captures what your speakers play (meeting apps, calls)</small>
          {outputs.length === 0 && <small>No output device found</small>}
          {outputs.map((device) => <label key={device.id} className="record-source"><input type="checkbox" checked={selected.includes(device.id)} onChange={() => onToggle(device.id)} /><span>{device.name}</span><small>{device.sample_rate ? `${Math.round(device.sample_rate / 1000)} kHz` : ''}</small></label>)}
        </div>
      </div>
    </details>
    <button className="record-button" onClick={onStart} disabled={disabled || selected.length === 0} title={selected.length === 0 ? 'Pick at least one audio source' : `Record from ${selected.length} source${selected.length === 1 ? '' : 's'}`}><Icon name="mic" size={15} />Record</button>
  </div>;
}
function Overview({ transcript, minutes, meeting, onOpenTranscript, onGenerate, generating }: { transcript: TranscriptSegment[]; minutes: Minutes; meeting: Meeting; onOpenTranscript: () => void; onGenerate: () => void; generating: boolean }) { return <div className="overview-page"><div className="overview-hero"><div className="overview-hero-copy"><BeaAvatar variant={transcript.length ? 'celebrating' : 'curious'} size={72} /><div><span className="page-kicker">Meeting overview</span><h2>Keep the thread moving.</h2><p>{transcript.length ? 'Your transcript is ready to review. Bea keeps every decision connected to its source timestamp.' : 'Add a recording or import a file to create a timestamped transcript and meeting ledger. Minutes need a transcript first.'}</p><div className="overview-hero-actions"><button className="primary" onClick={onOpenTranscript}>{transcript.length ? 'Review transcript' : 'Open transcript'} <Icon name="arrow-right" size={14} /></button>{transcript.length > 0 && <button className="secondary" onClick={onGenerate} disabled={generating}><Icon name="spark" size={14} />{generating ? 'Generating…' : 'Generate minutes'}</button>}</div></div></div></div><div className="overview-cards"><div><span className="overview-card-icon"><Icon name="waveform" size={17} /></span><strong>{transcript.length}</strong><span>Transcript segments</span><button onClick={onOpenTranscript}>View transcript <Icon name="arrow-right" size={12} /></button></div><div><span className="overview-card-icon"><Icon name="spark" size={17} /></span><strong>{minutes.decisions.length}</strong><span>Decisions captured</span><button onClick={onGenerate} disabled={!transcript.length || generating}>Generate from transcript <Icon name="arrow-right" size={12} /></button></div><div><span className="overview-card-icon"><Icon name="check" size={17} /></span><strong>{minutes.action_items.length}</strong><span>Open action items</span><button onClick={onGenerate} disabled={!transcript.length || generating}>Build meeting ledger <Icon name="arrow-right" size={12} /></button></div></div></div> }
function Inspector({ minutes, onGenerate, generating, hasTranscript }: { minutes: Minutes; onGenerate: () => void; generating: boolean; hasTranscript: boolean }) { return <aside className="workspace-inspector"><div className="inspector-block"><div className="inspector-block-heading"><span>Summary</span><Icon name="more" size={15} /></div><p className="inspector-summary">{minutes.summary || 'Generate minutes after reviewing the transcript to see a concise meeting summary here.'}</p><button className="inspector-link" onClick={onGenerate} disabled={!hasTranscript || generating}><Icon name="spark" size={14} />{generating ? 'Generating…' : hasTranscript ? 'Generate minutes' : 'Add a transcript first'} <Icon name="arrow-right" size={13} /></button></div><div className="inspector-block"><div className="inspector-block-heading"><span>Agenda</span><span className="inspector-count">{minutes.agenda.length}</span></div>{minutes.agenda.length ? minutes.agenda.map((item, index) => <div className="inspector-item" key={`agenda-${index}`}>{item.heading}{item.start_seconds != null && item.end_seconds != null && <small className="ledger-evidence">{mmss(item.start_seconds)}&ndash;{mmss(item.end_seconds)}</small>}</div>) : <p className="inspector-muted">No agenda extracted yet.</p>}</div><div className="inspector-block"><div className="inspector-block-heading"><span>Decisions</span><span className="inspector-count">{minutes.decisions.length}</span></div>{minutes.decisions.length ? minutes.decisions.slice(0, 3).map((item) => <div className="inspector-item" key={item.summary}>{item.summary}</div>) : <p className="inspector-muted">No decisions extracted yet.</p>}</div><div className="inspector-block"><div className="inspector-block-heading"><span>Action items</span><span className="inspector-count">{minutes.action_items.length}</span></div>{minutes.action_items.length ? minutes.action_items.slice(0, 3).map((item) => <div className="inspector-item" key={item.summary}>{item.summary}</div>) : <p className="inspector-muted">No action items extracted yet.</p>}</div></aside> }
type ExportFormatId = 'markdown' | 'pdf' | 'docx';
const EXPORT_FORMATS: Array<{ id: ExportFormatId; label: string; ext: string; hint: string; icon: IconName }> = [
  { id: 'markdown', label: 'Markdown', ext: '.md', hint: 'Plain text for notes apps', icon: 'file' },
  { id: 'pdf', label: 'PDF', ext: '.pdf', hint: 'Fixed layout for sharing', icon: 'download' },
  { id: 'docx', label: 'Word', ext: '.docx', hint: 'Editable document', icon: 'file' },
];
function ExportMenu({ onExport }: { onExport: (format: ExportFormatId) => void }) {
  const [open, setOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => { if (menuRef.current && !menuRef.current.contains(event.target as Node)) setOpen(false); };
    const onKeyDown = (event: KeyboardEvent) => { if (event.key === 'Escape') setOpen(false); };
    document.addEventListener('pointerdown', onPointerDown);
    document.addEventListener('keydown', onKeyDown);
    return () => { document.removeEventListener('pointerdown', onPointerDown); document.removeEventListener('keydown', onKeyDown); };
  }, [open]);
  return <div className="export-menu" ref={menuRef}><button type="button" className="secondary" aria-haspopup="menu" aria-expanded={open} onClick={() => setOpen((value) => !value)}><Icon name="download" size={14} />Export<Icon name="chevron-down" size={13} /></button>{open && <div className="export-menu-list" role="menu" aria-label="Export minutes as">{EXPORT_FORMATS.map((format) => <button key={format.id} type="button" className="export-menu-item" role="menuitem" onClick={() => { setOpen(false); onExport(format.id); }}><Icon name={format.icon} size={15} /><span><strong>{format.label} <small>{format.ext}</small></strong><small>{format.hint}</small></span></button>)}</div>}</div>;
}
function MinutesPanel({ minutes, onGenerate, onExport, generating, hasTranscript }: { minutes: Minutes; onGenerate: () => void; onExport: (format: 'markdown' | 'pdf' | 'docx') => void; generating: boolean; hasTranscript: boolean }) { return <div className="minutes-page"><div className="minutes-heading"><div><span className="page-kicker">Reviewable output</span><h2>Meeting minutes</h2><p>{hasTranscript ? 'Everything here is generated from your transcript — regenerate or export the version you are ready to share.' : 'Minutes are generated from the transcript — record or import media first.'}</p></div><div className="minutes-actions"><button className="secondary" onClick={onGenerate} disabled={!hasTranscript || generating}><Icon name="spark" size={14} />{generating ? 'Generating…' : 'Regenerate'}</button><ExportMenu onExport={onExport} /></div></div><div className="minutes-form"><section className="minutes-title-block"><span className="page-kicker">Title</span><h3>{minutes.title || 'Generated title appears here'}</h3></section><section className="minutes-summary-block"><span className="page-kicker">Summary</span><p>{minutes.summary || 'A concise summary will appear here after generation.'}</p></section><div className="minutes-columns"><section className="minutes-agenda"><h3>Agenda <span>{minutes.agenda.length}</span></h3>{minutes.agenda.length ? minutes.agenda.map((item, idx) => <div className="ledger-item" key={idx}><Icon name="list" size={14} /><div><strong>{item.heading}</strong>{item.start_seconds != null && item.end_seconds != null && <small className="ledger-evidence">{mmss(item.start_seconds)}&ndash;{mmss(item.end_seconds)}</small>}</div></div>) : <div className="ledger-empty">Agenda items are generated automatically from the transcript — click Regenerate to rebuild them.</div>}</section><section><h3>Decisions <span>{minutes.decisions.length}</span></h3>{minutes.decisions.length ? minutes.decisions.map((item, idx) => <div className="ledger-item" key={idx}><Icon name="check" size={14} /><div><strong>{item.summary}</strong>{item.evidence?.[0] && <small className="ledger-evidence">{mmss(item.evidence[0].start_seconds)}: &ldquo;{item.evidence[0].quote}&rdquo;</small>}</div></div>) : <div className="ledger-empty">Decisions will be linked to their transcript timestamps.</div>}</section><section><h3>Action items <span>{minutes.action_items.length}</span></h3>{minutes.action_items.length ? minutes.action_items.map((item, idx) => <div className="ledger-item" key={idx}><Icon name="clock" size={14} /><div><strong>{item.summary}</strong>{item.evidence?.[0] && <small className="ledger-evidence">{mmss(item.evidence[0].start_seconds)}: &ldquo;{item.evidence[0].quote}&rdquo;</small>}</div></div>) : <div className="ledger-empty">Action items will be linked to their transcript timestamps.</div>}</section>{minutes.unresolved && minutes.unresolved.length > 0 && <section><h3>Open / Unresolved <span>{minutes.unresolved.length}</span></h3>{minutes.unresolved.map((item, idx) => <div className="ledger-item" key={idx}><Icon name="spark" size={14} /><div><strong>{item.summary}</strong>{item.evidence?.[0] && <small className="ledger-evidence">{mmss(item.evidence[0].start_seconds)}: &ldquo;{item.evidence[0].quote}&rdquo;</small>}</div></div>)}</section>}{minutes.visual_observations && minutes.visual_observations.length > 0 && <section><h3>Visual evidence <span>{minutes.visual_observations.length}</span></h3>{minutes.visual_observations.map((observation, idx) => <div className="ledger-item" key={idx}><Icon name="image" size={14} /><div><strong>{observation}</strong></div></div>)}</section>}</div></div></div>; }
function EvidencePanel({ evidence }: { evidence: Array<{ start_seconds: number; end_seconds: number; quote: string; title?: string }> }) { return <div className="evidence-page"><div className="minutes-heading"><div><span className="page-kicker">Source trail</span><h2>Evidence</h2><p>Every extracted item stays connected to the moment it came from — the title states what the quote proves.</p></div><span className="section-count">{evidence.length} linked moments</span></div>{evidence.length ? <div className="evidence-list">{evidence.map((item, index) => <article key={`${item.start_seconds}-${index}`}><button className="evidence-time"><Icon name="play" size={12} />{mmss(item.start_seconds)}</button><div className="evidence-copy">{item.title && <strong>{item.title}</strong>}<p>{item.quote}</p></div><Icon name="chevron-right" size={15} /></article>)}</div> : <div className="empty-editor"><Icon name="image" size={25} /><p>Evidence links will appear after Bea extracts decisions, actions, or unresolved questions.</p></div>}</div> }

/// Per-block speaker picker: one button opens a checkbox list of every named
/// speaker. The first checked name is the primary speaker; further checked
/// names mark overlapping speech — one intuitive list instead of a select
/// plus a separate overlap popover.
function SpeakerPicker({ primary, extras, names, onApply }: { primary: number | null; extras: number[]; names: SpeakerName[]; onApply: (selection: number[]) => void }) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => { if (rootRef.current && !rootRef.current.contains(event.target as Node)) setOpen(false); };
    document.addEventListener('pointerdown', onPointerDown);
    return () => document.removeEventListener('pointerdown', onPointerDown);
  }, [open]);
  const selection = [primary, ...extras.filter((index) => index !== primary)].filter((value): value is number => value !== null);
  const toggle = (index: number) => onApply(selection.includes(index) ? selection.filter((value) => value !== index) : [...selection, index]);
  if (!names.length) return <span className="inspector-muted">Name speakers first</span>;
  return <div className="speaker-picker" ref={rootRef}>
    <button type="button" className={`speaker-picker-trigger${selection.length ? ' assigned' : ''}`} onClick={() => setOpen((value) => !value)} aria-haspopup="listbox" aria-expanded={open} title="Choose who is speaking — check several names for overlapping speech">
      <span className="speaker-picker-value">{selection.length ? speakerLabel(primary, names, extras) : 'Assign…'}</span>
      <Icon name="chevron-down" size={13} />
    </button>
    {open && <div className="speaker-picker-list" role="listbox" aria-label="Speakers on this block">
      {names.map((entry) => {
        const checked = selection.includes(entry.speaker_index);
        const isPrimary = selection[0] === entry.speaker_index;
        return <label key={entry.speaker_index} className={`speaker-picker-option${checked ? ' checked' : ''}`}>
          <input type="checkbox" checked={checked} onChange={() => toggle(entry.speaker_index)} />
          <span>{entry.name}{isPrimary && <small>main</small>}</span>
        </label>;
      })}
      <p className="speaker-picker-hint">First checked name = main speaker, further names = overlapping. Uncheck all to clear.</p>
    </div>}
  </div>;
}
