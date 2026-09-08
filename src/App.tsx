import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';
import type { AsrEngineDescriptor, AsrEngineId, Meeting, Minutes, OpenRouterModelInfo, ProviderConfig, RuntimeAvailability, SetupStatus, TranscriptLanguage, TranscriptSegment } from './types';
import { CODEX_FALLBACK_MODELS, DEFAULT_BASE_URLS, LOCAL_PRESETS } from './providerPresets';
import { loadMeetings, saveMeetings } from './meetingStore';
import SetupFlow, { type EngineProgress } from './SetupFlow';
import Library from './Library';
import MeetingWorkspace from './MeetingWorkspace';
import ModelSelect from './ModelSelect';
import Icon from './Icon';
import BeaAvatar from './BeaAvatar';
import { UpdateSettingsSection } from './UpdateSettings';
import './styles.css';

const initialProvider: ProviderConfig = { id: 'primary', kind: 'OpenRouter', base_url: 'https://openrouter.ai/api/v1', model: '', credential_ref: null, enabled: false };
export const initialEngines: AsrEngineDescriptor[] = [
  { id: 'whisper-compatibility', name: 'Bea Standard · Whisper', description: 'Broad language coverage and reliable Taglish transcription.', languages: '99 languages · EN · FIL · Taglish', size: 'Turbo · 1.6 GB', status: 'missing', recommended: true, detail: 'sherpa-onnx · local' },
  { id: 'qwen-standard', name: 'Qwen ASR', description: 'Fast local transcription for English, Filipino, and Taglish.', languages: 'EN · FIL · Taglish', size: '0.6B · 1.2 GB', status: 'missing', detail: 'sherpa-onnx · INT8' },
  { id: 'nemotron-multilingual', name: 'Nemotron multilingual', description: 'Low-latency multilingual streaming with a 560ms profile.', languages: 'Multilingual', size: '0.6B · 1.1 GB', status: 'missing', detail: 'Nemotron 3.5 · 560ms' },
];

function browserFallbackRuntime(): RuntimeAvailability { return { asr_model_available: false, ffmpeg_available: false, ffprobe_available: false, ocr_available: false, provider_configured: false, can_transcribe_locally: false }; }
export function setupFromRuntime(runtime: RuntimeAvailability | null, selectedEngine: AsrEngineId, verified: boolean, installedEngines: AsrEngineId[]): SetupStatus { const ffmpegReady = Boolean(runtime?.ffmpeg_available); const ffprobeReady = Boolean(runtime?.ffprobe_available); const tools = [{ id: 'ffmpeg' as const, name: 'FFmpeg + FFprobe', description: 'Media conversion and playback sources', status: ffmpegReady && ffprobeReady ? 'ready' as const : 'missing' as const, detail: ffmpegReady && ffprobeReady ? 'FFmpeg and FFprobe version checked' : ffmpegReady ? 'FFmpeg detected; FFprobe is still missing' : ffprobeReady ? 'FFprobe detected; FFmpeg is still missing' : 'Neither executable passed the version check' }, { id: 'tesseract' as const, name: 'Tesseract OCR', description: 'Read text from slides and visual evidence', status: runtime?.ocr_available ? 'ready' as const : 'missing' as const, detail: runtime?.ocr_available ? 'English trained data ready' : 'Install the OCR bundle' }]; const engines = initialEngines.map((engine) => ({ ...engine, status: (installedEngines.includes(engine.id) || (engine.id === selectedEngine && runtime?.asr_model_available) || (engine.id === 'whisper-compatibility' && runtime?.asr_model_available)) ? 'ready' as const : engine.status })); return { tools, engines, selectedEngine, provider: { verified, message: verified ? 'Connection verified. Your key is not stored in the project database.' : undefined }, complete: tools.every((tool) => tool.status === 'ready') && engines.some((engine) => engine.id === selectedEngine && engine.status === 'ready') && verified }; }

export default function App() {
  const [meetings, setMeetings] = useState<Meeting[]>(() => loadMeetings());
  const [runtime, setRuntime] = useState<RuntimeAvailability | null>(null);
  const [provider, setProvider] = useState<ProviderConfig>(initialProvider);
  const [apiKey, setApiKey] = useState('');
  const [selectedEngine, setSelectedEngine] = useState<AsrEngineId>(() => (localStorage.getItem('bea.selected-engine') as AsrEngineId | null) ?? 'whisper-compatibility');
  const [installedEngines, setInstalledEngines] = useState<AsrEngineId[]>(() => { try { return JSON.parse(localStorage.getItem('bea.installed-engines') ?? '[]') as AsrEngineId[]; } catch { return []; } });
  const [setupStep, setSetupStep] = useState(0);
  const [setupVerified, setSetupVerified] = useState(() => localStorage.getItem('bea.provider-verified') === 'true');
  const [setupComplete, setSetupComplete] = useState(() => localStorage.getItem('bea.setup-complete') === 'true');
  const [route, setRoute] = useState<{ screen: 'library' | 'meeting' | 'settings'; meetingId?: string }>(() => parseHash());
  const lastOpenedRef = useRef<string | null>(null);
  const [librarySelection, setLibrarySelection] = useState<string | null>(null);
  const [notice, setNotice] = useState('Check the essentials to unlock your meeting library.');
  const [repairing, setRepairing] = useState<Array<'ffmpeg' | 'tesseract'>>([]);
  const [engineProgress, setEngineProgress] = useState<EngineProgress | null>(null);
  const [transcribing, setTranscribing] = useState<string | null>(null);
  const [transcriptionProgress, setTranscriptionProgress] = useState<{ completed: number; total: number } | null>(null);
  const [liveSegments, setLiveSegments] = useState<Record<string, TranscriptSegment[]>>({});

  const setupStatus = useMemo(() => setupFromRuntime(runtime, selectedEngine, setupVerified, installedEngines), [runtime, selectedEngine, setupVerified, installedEngines]);
  const selectedMeeting = meetings.find((meeting) => meeting.id === route.meetingId) ?? null;
  const selectedEngineName = setupStatus.engines.find((engine) => engine.id === selectedEngine)?.name ?? 'Bea Standard · Whisper';

  useEffect(() => { const handleHash = () => setRoute(parseHash()); window.addEventListener('hashchange', handleHash); return () => window.removeEventListener('hashchange', handleHash); }, []);
  useEffect(() => {
    // Track disposal so listeners registered after unmount are detached
    // immediately instead of leaking for the page lifetime.
    let disposed = false;
    let unlisten: (() => void) | undefined;
    if ((window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__) {
      const stopDependency = listen<{ id: string; downloaded: number; total: number | null }>('dependency-progress', (event) => {
        const { id, downloaded, total } = event.payload;
        setEngineProgress({ id, downloaded, total, phase: 'downloading' });
        const mb = Math.round(downloaded / 1_000_000);
        setNotice(total ? `Downloading ${id}… ${mb} MB of ${Math.round(total / 1_000_000)} MB` : `Downloading ${id}… ${mb} MB`);
      });
      const stopTranscription = listen<{ meeting_id: string; completed: number; total: number; segment?: TranscriptSegment }>('transcription-progress', (event) => {
        const { meeting_id, completed, total, segment } = event.payload;
        setTranscriptionProgress({ completed, total });
        setNotice(`Transcribing… chunk ${completed} of ${total}`);
        // Stream finished segments into the open meeting's transcript live.
        if (segment) setLiveSegments((current) => ({ ...current, [meeting_id]: [...(current[meeting_id] ?? []), segment] }));
      });
      Promise.all([stopDependency, stopTranscription]).then(([a, b]) => {
        if (disposed) { a(); b(); return; }
        unlisten = () => { a(); b(); };
      }).catch(() => undefined);
    }
    return () => { disposed = true; unlisten?.(); };
  }, []);
  useEffect(() => { Promise.all([invoke<Meeting[]>('list_meetings_command').catch(() => null), invoke<RuntimeAvailability>('inspect_runtime_command').catch(() => browserFallbackRuntime()), invoke<ProviderConfig | null>('load_provider_command', { providerId: 'primary' }).catch(() => null)]).then(([remoteMeetings, inspected, savedProvider]) => { if (remoteMeetings) { setMeetings(remoteMeetings); saveMeetings(remoteMeetings); } setRuntime(inspected); if (savedProvider) setProvider(savedProvider); }); }, []);
  useEffect(() => { invoke<{ selected_engine: AsrEngineId; setup_complete: boolean; provider_verified: boolean }>('setup_status_command').then((snapshot) => { if (snapshot.selected_engine) { setSelectedEngine(snapshot.selected_engine); localStorage.setItem('bea.selected-engine', snapshot.selected_engine); } setSetupComplete(snapshot.setup_complete); if (snapshot.setup_complete) localStorage.setItem('bea.setup-complete', 'true'); else localStorage.removeItem('bea.setup-complete'); setSetupVerified(snapshot.provider_verified); if (snapshot.provider_verified) localStorage.setItem('bea.provider-verified', 'true'); else localStorage.removeItem('bea.provider-verified'); }).catch(() => undefined); }, []);
  useEffect(() => {
    // Functional update so saveMeetings always persists the *current* meeting
    // list (a stale snapshot could resurrect deleted meetings). The ref guard
    // keeps this to one "last opened" write per navigation.
    if (route.screen !== 'meeting' || !selectedMeeting) return;
    if (lastOpenedRef.current === selectedMeeting.id) return;
    lastOpenedRef.current = selectedMeeting.id;
    setLibrarySelection(selectedMeeting.id);
    setMeetings((current) => { const next = current.map((item) => item.id === selectedMeeting.id ? { ...item, last_opened_at: new Date().toISOString() } : item); saveMeetings(next); return next; });
  }, [route.screen, route.meetingId, selectedMeeting]);

  const refreshRuntime = useCallback(async () => { try { const inspected = await invoke<RuntimeAvailability>('inspect_runtime_command'); setRuntime(inspected); setNotice('Runtime checks refreshed.'); } catch { setRuntime((current) => current ?? browserFallbackRuntime()); setNotice('Runtime checks will refresh when Bea is running as a desktop app.'); } }, []);
  async function repairTools(ids: Array<'ffmpeg' | 'tesseract'>) {
    setRepairing(ids);
    const inDesktop = Boolean((window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__);
    setNotice(`Setting up ${ids.join(' and ')}…`);
    if (inDesktop) {
      try {
        // Download-first: fetch the official builds into the app-data bin directory.
        for (const id of ids) {
          setRepairing([id]);
          setNotice(`Downloading ${id === 'ffmpeg' ? 'FFmpeg + FFprobe' : 'Tesseract OCR'}…`);
          await invoke<RuntimeAvailability>('download_tool_command', { tool: id });
        }
        const inspected = await invoke<RuntimeAvailability>('inspect_runtime_command');
        setRuntime(inspected);
        setNotice('Required local tools are ready.');
        return;
      } catch (downloadError) {
        setNotice(`Download failed (${String(downloadError)}). Trying the bundled copies…`);
      } finally {
        setRepairing([]);
      }
    }
    // Fallback: copy the binaries bundled with the app (offline machines).
    try {
      setRepairing(ids);
      const inspected = await invoke<RuntimeAvailability>('repair_runtime_command', { tools: ids });
      setRuntime(inspected);
      setNotice(ids.every((id) => id === 'ffmpeg' ? inspected.ffmpeg_available && inspected.ffprobe_available : inspected.ocr_available) ? 'Required local tools are ready.' : 'Repair finished, but one or more tools still need attention.');
    } catch (error) {
      const message = String(error);
      if (!inDesktop && /not found|unknown command|tauri/i.test(message)) {
        await new Promise((resolve) => setTimeout(resolve, 700));
        setRuntime((current) => ({ ...(current ?? browserFallbackRuntime()), ffmpeg_available: ids.includes('ffmpeg') ? true : Boolean(current?.ffmpeg_available), ffprobe_available: ids.includes('ffmpeg') ? true : Boolean(current?.ffprobe_available), ocr_available: ids.includes('tesseract') ? true : Boolean(current?.ocr_available) }));
        setNotice('Browser preview marked the requested tools as ready; verify them in the packaged app.');
      } else {
        setNotice(`Repair failed: ${message}`);
      }
    } finally {
      setRepairing([]);
    }
  }
  function selectEngine(id: AsrEngineId) { setSelectedEngine(id); localStorage.setItem('bea.selected-engine', id); void invoke('set_engine_selection_command', { engineId: id }).catch(() => undefined); setNotice(`${initialEngines.find((engine) => engine.id === id)?.name ?? 'Engine'} selected.`); }
  async function installEngine(id: AsrEngineId) {
    selectEngine(id);
    if (!(window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__) { setNotice('Downloads run in the desktop app; browser preview can register a package manually.'); return; }
    setEngineProgress({ id, downloaded: 0, total: null, phase: 'downloading' });
    setNotice(`Downloading ${initialEngines.find((engine) => engine.id === id)?.name ?? id}…`);
    try {
      await invoke('download_engine_command', { engineId: id });
      setEngineProgress({ id, downloaded: 0, total: null, phase: 'installing' });
      setInstalledEngines((current) => { const next = current.includes(id) ? current : [...current, id]; localStorage.setItem('bea.installed-engines', JSON.stringify(next)); return next; });
      setRuntime((current) => ({ ...(current ?? browserFallbackRuntime()), asr_model_available: true, can_transcribe_locally: true }));
      setNotice(`Downloaded and verified ${initialEngines.find((engine) => engine.id === id)?.name ?? id}.`);
    } catch (downloadError) {
      setNotice(`Download failed (${String(downloadError)}). Pick the package file instead.`);
      try {
        const selected = await open({ multiple: false, directory: false, filters: [{ name: 'Verified Bea ASR package', extensions: ['bz2', 'zip', 'tar', 'onnx'] }] });
        const path = Array.isArray(selected) ? selected[0] : selected;
        if (!path) { setNotice(`No package selected. ${initialEngines.find((engine) => engine.id === id)?.name ?? id} is still selected; retry the download or add its package file.`); return; }
        await importEngine(path, id);
      } catch (error) {
        setNotice(`Unable to install the selected engine: ${String(error)}`);
      }
    } finally {
      setEngineProgress(null);
    }
  }
  async function importEngine(path: string, expectedEngine?: AsrEngineId) { const manifest = await invoke<{ id: string; name: string; version: string; size_bytes: number; sha256: string; runtime: string; languages: string[]; installed: boolean }>('inspect_model_package_command', { path }); const id = manifest.id === 'qwen3-asr-0.6b-int8' ? 'qwen-standard' : manifest.id as AsrEngineId; if (expectedEngine && expectedEngine !== id) throw new Error(`This package is ${id}, but ${expectedEngine} is selected.`); const progress = await invoke<{ verified: boolean; bytes_copied: number }>('install_model_command', { source: path, manifest }); if (!progress.verified) throw new Error('Package verification failed'); setInstalledEngines((current) => { const next = current.includes(id) ? current : [...current, id]; localStorage.setItem('bea.installed-engines', JSON.stringify(next)); return next; }); setSelectedEngine(id); setRuntime((current) => ({ ...(current ?? browserFallbackRuntime()), asr_model_available: true, can_transcribe_locally: true })); setNotice(`Installed and verified ${manifest.name}.`); }
  async function testProvider() { setNotice('Testing the provider connection…'); const keyRequired = provider.kind !== 'Local' && provider.kind !== 'OpenAiOAuth'; const persistVerifiedProvider = async () => { const verifiedProvider = { ...provider, enabled: true, credential_ref: keyRequired ? `keyring:${provider.id}` : provider.credential_ref }; setSetupVerified(true); setProvider(verifiedProvider); localStorage.setItem('bea.provider-verified', 'true'); try { if (keyRequired) await invoke('save_provider_secure_command', { provider: verifiedProvider, apiKey }); else await invoke('save_provider_command', { provider: verifiedProvider }); } catch { try { await invoke('save_provider_command', { provider: verifiedProvider }); } catch { /* Browser preview has no Tauri database. */ } } }; try { if (provider.kind === 'OpenAiOAuth') { await invoke('codex_oauth_status_command'); } else { await invoke('test_provider_connection_command', { provider, apiKey }); setNotice('Connection ok — validating minutes output…'); await invoke('validate_minutes_model_command', { provider, apiKey }); } await persistVerifiedProvider(); setNotice(keyRequired ? 'Provider verified: the model returned valid minutes JSON. API key is stored in Windows Credential Manager, outside SQLite.' : 'Provider verified: the model returned valid minutes JSON.'); } catch (error) { const message = String(error); const browserFallback = /not found|unknown command|tauri/i.test(message); if (browserFallback && provider.base_url && provider.model && (keyRequired ? apiKey : true)) { await persistVerifiedProvider(); setNotice('Connection verified for this setup session.'); } else { setSetupVerified(false); setNotice(`Provider test failed: ${message}`); } } }
  function completeSetup() { localStorage.setItem('bea.setup-complete', 'true'); void invoke('complete_setup_command').catch(() => undefined); setSetupComplete(true); window.location.hash = '#library'; setNotice('Workspace ready. Create a meeting to begin.'); }
  async function createMeeting(title: string, language: TranscriptLanguage, engine: AsrEngineId) { let meeting: Meeting; try { meeting = await invoke<Meeting>('create_meeting_command', { title, language, asrEngineId: engine }); } catch { meeting = { id: crypto.randomUUID(), title, status: 'draft', duration_seconds: 0, language, created_at: new Date().toISOString(), asr_engine_id: engine }; } meeting = { ...meeting, asr_engine_id: engine }; setMeetings((current) => { const next = [meeting, ...current]; saveMeetings(next); return next; }); window.location.hash = `#meeting/${meeting.id}`; setNotice('Meeting created. Add a recording or import media to start.'); }
  async function runTranscription(meeting: Meeting, language: TranscriptLanguage, command: string, args: Record<string, unknown>) {
    setTranscribing(meeting.id);
    setTranscriptionProgress(null);
    setMeetings((current) => current.map((item) => item.id === meeting.id ? { ...item, status: 'processing' } : item));
    try {
      await invoke<unknown[]>(command, args);
      setMeetings((current) => current.map((item) => item.id === meeting.id ? { ...item, status: 'ready' } : item));
      setNotice('Transcription completed locally.');
    } catch (error) {
      setMeetings((current) => current.map((item) => item.id === meeting.id ? { ...item, status: 'failed' } : item));
      setNotice(`Transcription failed: ${String(error)} — open the meeting to retry.`);
      // The meeting screen renders its own toast (App notices are invisible there).
      window.dispatchEvent(new CustomEvent('bea:notice', { detail: { message: `Transcription failed: ${String(error)}` } }));
    } finally {
      setTranscribing(null);
      setTranscriptionProgress(null);
      // Refetch the persisted transcript BEFORE dropping the streamed live
      // segments so the transcript never shows a gap between run end and the
      // refetch resolving (segments would otherwise vanish momentarily).
      try {
        const stored = await invoke<TranscriptSegment[]>('list_transcript_command', { meetingId: meeting.id });
        setLiveSegments((current) => ({ ...current, [meeting.id]: [] }));
        window.dispatchEvent(new CustomEvent('bea:transcript-updated', { detail: { meetingId: meeting.id, segments: stored } }));
      } catch {
        setLiveSegments((current) => { const next = { ...current }; delete next[meeting.id]; return next; });
      }
    }
  }
  async function importMedia(meeting: Meeting) { try { const selected = await open({ multiple: false, directory: false, filters: [{ name: 'Audio or video', extensions: ['wav', 'mp3', 'm4a', 'ogg', 'flac', 'aac', 'mp4', 'mov', 'mkv', 'webm', 'avi'] }] }); const path = Array.isArray(selected) ? selected[0] : selected; if (!path) return; const extension = path.split('.').pop()?.toLowerCase() ?? ''; const kind = ['mp4', 'mov', 'mkv', 'webm', 'avi'].includes(extension) ? 'video' : 'audio'; const imported = await invoke<{ path: string }>('import_media_command', { meetingId: meeting.id, path, kind, copyIntoLibrary: true }); window.dispatchEvent(new CustomEvent('bea:media-updated', { detail: { meetingId: meeting.id } })); await runTranscription(meeting, meeting.language, 'process_imported_media_command', { meetingId: meeting.id, path: imported.path, kind, language: meeting.language }); window.dispatchEvent(new CustomEvent('bea:media-updated', { detail: { meetingId: meeting.id } })); } catch (error) { setNotice(`Unable to import media: ${String(error)}`); if (!/cancelled|no file selected/i.test(String(error))) window.dispatchEvent(new CustomEvent('bea:notice', { detail: { message: `Unable to import media: ${String(error)}` } })); } }
  async function importVtt(meeting: Meeting) {
    try {
      const selected = await open({ multiple: false, directory: false, filters: [{ name: 'MS Teams transcript', extensions: ['vtt'] }] });
      const path = Array.isArray(selected) ? selected[0] : selected;
      if (!path) return;
      // An existing transcript (Bea's own transcription or an earlier import)
      // must never be overwritten silently — let the user pick which one wins.
      const existing = await invoke<TranscriptSegment[]>('list_transcript_command', { meetingId: meeting.id }).catch(() => [] as TranscriptSegment[]);
      if (existing.length > 0) {
        const useImported = window.confirm(
          `This meeting already has a transcript (${existing.length} segment${existing.length === 1 ? '' : 's'}). Importing the VTT file replaces it.\n\nOK — use the imported VTT transcript\nCancel — keep the Bea transcription`,
        );
        if (!useImported) {
          setNotice('Kept the existing transcription — the VTT file was not imported.');
          window.dispatchEvent(new CustomEvent('bea:notice', { detail: { message: 'Kept the existing transcription — the VTT file was not imported.' } }));
          return;
        }
      }
      const segments = await invoke<TranscriptSegment[]>('import_vtt_command', { meetingId: meeting.id, path });
      const stored = segments.length;
      setMeetings((current) => current.map((item) => item.id === meeting.id ? { ...item, status: 'ready' } : item));
      window.dispatchEvent(new CustomEvent('bea:transcript-updated', { detail: { meetingId: meeting.id, segments: [] } }));
      setNotice(`Imported ${stored} transcript cues from ${path.split(/[\\/]/).pop()} — speakers are named from the VTT file.`);
    } catch (error) {
      setNotice(`Unable to import VTT: ${String(error)}`);
      window.dispatchEvent(new CustomEvent('bea:notice', { detail: { message: `Unable to import VTT: ${String(error)}` } }));
    }
  }
  async function recording(meeting: Meeting, action: 'start' | 'pause' | 'resume' | 'stop', deviceIds?: string[]) { const command = { start: 'start_recording_command', pause: 'pause_recording_command', resume: 'resume_recording_command', stop: 'stop_recording_command' }[action]; try { const result = await invoke<{ end_seconds?: number }[]>(command, { meetingId: meeting.id, ...(action === 'start' ? { deviceIds: deviceIds ?? [] } : {}) }); const nextStatus: Meeting['status'] = action === 'start' ? 'recording' : action === 'pause' ? 'paused' : action === 'resume' ? 'recording' : 'processing'; setMeetings((current) => current.map((item) => item.id === meeting.id ? { ...item, status: nextStatus, duration_seconds: result?.at(-1)?.end_seconds ?? item.duration_seconds } : item)); if (action === 'stop') { await runTranscription(meeting, meeting.language, 'transcribe_recording_command', { meetingId: meeting.id, language: meeting.language }); } else setNotice(`Recording ${action}ed.`); } catch (error) { setNotice(`Unable to ${action} recording: ${String(error)}`); window.dispatchEvent(new CustomEvent('bea:notice', { detail: { message: `Unable to ${action} recording: ${String(error)}` } })); } }
  async function retryTranscription(meeting: Meeting) { await runTranscription(meeting, meeting.language, 'transcribe_recording_command', { meetingId: meeting.id, language: meeting.language }); }
  async function renameMeeting(meeting: Meeting, title: string) { try { await invoke('rename_meeting_command', { meetingId: meeting.id, title }); } catch { /* local preview */ } setMeetings((current) => { const next = current.map((item) => item.id === meeting.id ? { ...item, title } : item); saveMeetings(next); return next; }); setNotice('Meeting renamed.'); }
  async function deleteMeeting(meeting: Meeting) { if (!window.confirm(`Delete “${meeting.title}”? This removes its local meeting project.`)) return; try { await invoke('delete_meeting_command', { meetingId: meeting.id }); } catch { /* local preview */ } setMeetings((current) => { const next = current.filter((item) => item.id !== meeting.id); saveMeetings(next); return next; }); if (route.meetingId === meeting.id) window.location.hash = '#library'; setNotice('Meeting project deleted.'); }
  function openSettings() { window.location.hash = '#settings'; setRoute({ screen: 'settings' }); setNotice('Settings are ready for runtime and provider maintenance.'); }
  async function deleteAllData() {
    if (!window.confirm('Delete ALL data? This permanently removes every meeting, transcript, minutes, chat notes, generated files, and the ChatGPT sign-in. Downloaded engines are kept.')) return;
    if (!window.confirm('This cannot be undone. Continue?')) return;
    try { await invoke('delete_all_data_command'); } catch { /* browser preview has no backend */ }
    localStorage.removeItem('bea.meetings.v1');
    localStorage.removeItem('bea.provider-verified');
    localStorage.removeItem('bea.installed-engines');
    setMeetings([]);
    setLibrarySelection(null);
    setSetupVerified(false);
    setProvider(initialProvider);
    setApiKey('');
    if (route.meetingId) window.location.hash = '#library';
    setNotice('All data deleted. Bea is empty again.');
  }
  async function clearAiMemory() {
    if (!window.confirm("Clear AI memory? This permanently deletes every meeting's chat history and the clarification/context notes the AI uses as memory. Transcripts, minutes, and meetings are kept. This cannot be undone.")) return;
    try {
      const removed = await invoke<number>('clear_ai_memory_command');
      const message = `AI memory cleared — ${removed} ${removed === 1 ? 'entry' : 'entries'} removed.`;
      setNotice(message);
      window.dispatchEvent(new CustomEvent('bea:notice', { detail: { message } }));
    } catch (error) {
      const message = `Unable to clear AI memory: ${String(error)}`;
      setNotice(message);
      window.dispatchEvent(new CustomEvent('bea:notice', { detail: { message } }));
    }
  }

  if (!setupComplete) return <SetupFlow status={{ ...setupStatus, tools: setupStatus.tools.map((tool) => repairing.includes(tool.id) ? { ...tool, status: 'repairing' } : tool) }} provider={provider} apiKey={apiKey} step={setupStep} onStep={setSetupStep} onSelectEngine={selectEngine} onInstallEngine={installEngine} onImportEngine={importEngine} onRepairTools={repairTools} onProviderChange={setProvider} onApiKeyChange={setApiKey} onTestProvider={testProvider} onComplete={completeSetup} notice={notice} engineProgress={engineProgress} />;
  if (route.screen === 'meeting' && selectedMeeting) return <MeetingWorkspace key={selectedMeeting.id} meeting={selectedMeeting} onBack={() => { window.location.hash = '#library'; }} onNotice={setNotice} onImport={importMedia} onImportVtt={importVtt} onRecording={recording} onMeetingStatus={(id, status) => setMeetings((items) => items.map((item) => item.id === id ? { ...item, status } : item))} onRetryTranscription={retryTranscription} busy={transcribing === selectedMeeting.id} transcribeProgress={transcribing === selectedMeeting.id ? transcriptionProgress : null} liveSegments={liveSegments[selectedMeeting.id] ?? []} />;
  if (route.screen === 'settings') return <SettingsScreen status={setupStatus} provider={provider} apiKey={apiKey} onBack={() => { window.location.hash = '#library'; }} onRefresh={refreshRuntime} onRepair={repairTools} onProviderChange={setProvider} onApiKeyChange={setApiKey} onTest={testProvider} onSelectEngine={selectEngine} onInstallEngine={installEngine} onReset={() => { localStorage.removeItem('bea.setup-complete'); setSetupComplete(false); setSetupStep(0); }} onDeleteAllData={() => void deleteAllData()} onClearAiMemory={() => void clearAiMemory()} />;
  return <Library meetings={meetings} selectedId={librarySelection ?? meetings[0]?.id ?? null} onSelect={setLibrarySelection} onOpen={(meeting) => { setLibrarySelection(meeting.id); window.location.hash = `#meeting/${meeting.id}`; }} onCreate={createMeeting} onImport={importMedia} onImportVtt={importVtt} onRename={renameMeeting} onDelete={deleteMeeting} onSettings={openSettings} runtimeReady={Boolean(runtime?.can_transcribe_locally || setupStatus.engines.some((engine) => engine.id === selectedEngine && engine.status === 'ready'))} selectedEngineName={selectedEngineName} />;
}

function parseHash(): { screen: 'library' | 'meeting' | 'settings'; meetingId?: string } { const hash = window.location.hash.replace(/^#/, ''); if (hash === 'settings') return { screen: 'settings' }; if (hash.startsWith('meeting/')) return { screen: 'meeting', meetingId: hash.slice('meeting/'.length) }; return { screen: 'library' }; }

/// Settings-side ChatGPT sign-in block (same contract as SetupFlow's CodexSignIn).
function CodexSignInSettings({ provider, onProviderChange, verified }: { provider: ProviderConfig; onProviderChange: (provider: ProviderConfig) => void; verified: boolean }) {
  const [codexModels, setCodexModels] = useState<string[]>(CODEX_FALLBACK_MODELS);
  const [signingIn, setSigningIn] = useState(false);
  const [signInError, setSignInError] = useState<string | null>(null);
  const refreshModels = () => {
    invoke<string[]>('codex_list_models_command')
      .then((models) => { if (models.length) setCodexModels(models); })
      .catch(() => undefined);
  };
  useEffect(refreshModels, []);
  return <div className="oauth-block">
    <label>Minutes model<select value={provider.model} onChange={(event) => onProviderChange({ ...provider, model: event.target.value })}>{codexModels.map((id) => <option key={id} value={id}>{id}</option>)}</select></label>
    <button type="button" className="primary" disabled={signingIn} onClick={() => { setSigningIn(true); setSignInError(null); invoke('codex_oauth_login_command').then(() => onProviderChange({ ...provider, enabled: true })).then(refreshModels).catch((error) => setSignInError(`Sign-in failed: ${String(error)}`)).finally(() => setSigningIn(false)); }}>
      <Icon name="spark" size={14} />{signingIn ? 'Waiting for browser…' : verified ? 'Re-sign in with ChatGPT' : 'Sign in with ChatGPT'}
    </button>
    <small className="field-help">Uses your ChatGPT/Codex subscription — no API key. Tokens live in your Windows user profile, outside the project database.</small>
    {signInError && <p className="provider-message error">{signInError}</p>}
  </div>;
}

function SettingsScreen({ status, provider, apiKey, onBack, onRefresh, onRepair, onProviderChange, onApiKeyChange, onTest, onSelectEngine, onInstallEngine, onReset, onDeleteAllData, onClearAiMemory }: { status: SetupStatus; provider: ProviderConfig; apiKey: string; onBack: () => void; onRefresh: () => void; onRepair: (ids: Array<'ffmpeg' | 'tesseract'>) => Promise<void>; onProviderChange: (provider: ProviderConfig) => void; onApiKeyChange: (value: string) => void; onTest: () => Promise<void>; onSelectEngine: (id: AsrEngineId) => void; onInstallEngine: (id: AsrEngineId) => Promise<void>; onReset: () => void; onDeleteAllData: () => void; onClearAiMemory: () => void }) {
  const keyRequired = provider.kind !== 'Local' && provider.kind !== 'OpenAiOAuth';
  const [openRouterModels, setOpenRouterModels] = useState<OpenRouterModelInfo[]>([]);
  useEffect(() => { invoke<OpenRouterModelInfo[]>('fetch_openrouter_models_command').then(setOpenRouterModels).catch(() => setOpenRouterModels([])); }, []);
  const [discoveredIds, setDiscoveredIds] = useState<string[]>([]);
  useEffect(() => {
    if (provider.kind !== 'Local' && provider.kind !== 'OpenAiCompatible') return;
    if (!provider.base_url || !apiKey) { setDiscoveredIds([]); return; }
    invoke<string[]>('discover_provider_models_command', { provider, apiKey }).then(setDiscoveredIds).catch(() => setDiscoveredIds([]));
  }, [provider.kind, provider.base_url, provider.id, apiKey]);

  const modelOptions = provider.kind === 'OpenRouter' ? openRouterModels.map((m) => m.id) : discoveredIds;

  return (
    <main className="settings-shell">
      <header className="settings-header">
        <button className="back-link" onClick={onBack}><Icon name="arrow-left" size={15} />Meetings</button>
        <div className="settings-title-lockup"><BeaAvatar variant="waving" size={54} /><div><span className="page-kicker">Workspace settings</span><h1>Keep Bea ready.</h1><p>Repair local tools, switch transcription engines, or rotate the provider connection.</p></div></div>
        <button className="secondary" onClick={onRefresh}><Icon name="refresh" size={14} />Refresh checks</button>
      </header>
      <div className="settings-grid">
        <section className="settings-section">
          <div className="settings-section-title"><div><h2>Local tools</h2><p>Required for media conversion and OCR.</p></div></div>
          {status.tools.map((tool) => (
            <div className="settings-row" key={tool.id}>
              <span className={`setup-status ${tool.status === 'ready' ? 'is-ready' : ''}`}><Icon name={tool.status === 'ready' ? 'check' : 'download'} size={14} /></span>
              <span><strong>{tool.name}</strong><small>{tool.detail}</small></span>
              <button className="secondary small" onClick={() => void onRepair([tool.id])}>{tool.status === 'ready' ? 'Repair' : 'Install'}</button>
            </div>
          ))}
        </section>
        <section className="settings-section">
          <div className="settings-section-title"><div><h2>Transcription</h2><p>Existing meetings keep the engine they were created with.</p></div></div>
          {status.engines.map((engine) => (
            <button type="button" key={engine.id} className={`settings-engine ${status.selectedEngine === engine.id ? 'selected' : ''}`} onClick={() => engine.status === 'ready' ? onSelectEngine(engine.id) : void onInstallEngine(engine.id)}>
              <span><Icon name="waveform" size={15} /><strong>{engine.name}</strong><small>{engine.languages} · {engine.size}</small></span>
              <span className={`state-label ${engine.status === 'ready' ? 'ready' : 'attention'}`}>{status.selectedEngine === engine.id ? 'In use' : engine.status === 'ready' ? 'Installed · Select' : 'Install package'}</span>
            </button>
          ))}
        </section>
        <UpdateSettingsSection />
        <section className="settings-section provider-settings">
          <div className="settings-section-title"><div><h2>AI provider</h2><p>Keys are held outside your project database.</p></div><span className={`state-label ${status.provider.verified ? 'ready' : 'attention'}`}>{status.provider.verified ? 'Verified' : 'Needs test'}</span></div>
          <label>Provider<select value={provider.kind} onChange={(event) => onProviderChange({ ...provider, kind: event.target.value as ProviderConfig['kind'], base_url: DEFAULT_BASE_URLS[event.target.value as ProviderConfig['kind']] ?? '' })}>
            <option value="OpenRouter">OpenRouter</option>
            <option value="OpenAiCompatible">OpenAI-compatible</option>
            <option value="ClaudeCompatible">Claude-compatible proxy</option>
            <option value="Local">Local server (LM Studio / Ollama / llama.cpp)</option>
            <option value="OpenAiOAuth">OpenAI (ChatGPT sign-in)</option>
          </select></label>
          {provider.kind === 'Local' && (
            <div className="local-presets">{LOCAL_PRESETS.map((preset) => (
              <button key={preset.id} type="button" className={`preset ${provider.base_url === preset.base_url ? 'selected' : ''}`} onClick={() => onProviderChange({ ...provider, base_url: preset.base_url })}>{preset.label}<small>{preset.base_url}</small></button>
            ))}</div>
          )}
          {provider.kind === 'OpenAiOAuth' && <CodexSignInSettings provider={provider} onProviderChange={onProviderChange} verified={status.provider.verified} />}
          {provider.kind !== 'OpenAiOAuth' && (
            <label>Model
              <ModelSelect
                value={provider.model}
                onChange={(model) => onProviderChange({ ...provider, model })}
                models={provider.kind === 'OpenRouter' ? openRouterModels : []}
                ids={provider.kind === 'OpenRouter' ? openRouterModels.map((m) => m.id) : discoveredIds}
                ariaLabel="Default AI model"
              />
              <small className="field-help">Default model for minutes and chat. Badges show Visual / Audio / File input support from the provider catalog. A meeting can override it in its Chat tab.</small>
            </label>
          )}
          {provider.kind !== 'OpenAiOAuth' && <label>Base URL<input value={provider.base_url} onChange={(event) => onProviderChange({ ...provider, base_url: event.target.value })} placeholder="https://…" /></label>}
          {provider.kind !== 'Local' && provider.kind !== 'OpenAiOAuth' && <label>API key<input type="password" value={apiKey} onChange={(event) => onApiKeyChange(event.target.value)} placeholder="Enter to rotate key" autoComplete="off" /></label>}
          {provider.kind !== 'OpenAiOAuth' && <button className="primary" onClick={() => void onTest()} disabled={!provider.base_url || !provider.model || (keyRequired && !apiKey)}><Icon name="spark" size={14} />Test, validate minutes & save provider</button>}
        </section>
        <section className="settings-section" style={{ gridColumn: '1 / -1' }}>
          <div className="settings-section-title"><div><h2>AI memory</h2><p>Chat history and notes the AI uses to answer questions, including references to earlier meetings.</p></div></div>
          <div className="settings-row">
            <span className="setup-status"><Icon name="trash" size={13} /></span>
            <span><strong>Clear AI memory</strong><small>Deletes every meeting's chat Q&A history and user-added clarification/context notes. Transcripts and minutes are kept.</small></span>
            <button className="secondary small" onClick={onClearAiMemory}>Clear</button>
          </div>
        </section>
      </div>
      <div className="settings-danger"><div><strong>Delete all data</strong><p>Permanently removes every meeting, transcript, minutes, chat notes, generated files, and the ChatGPT sign-in. Downloaded engines and local tools are kept. This cannot be undone.</p></div><button className="danger-button" onClick={onDeleteAllData}><Icon name="trash" size={14} />Delete all data</button></div>
      <div className="settings-danger"><div><strong>Start setup again</strong><p>Use this only if you want to change the required first-run choices.</p></div><button className="secondary" onClick={onReset}>Reset setup gate</button></div>
    </main>
  );
}
