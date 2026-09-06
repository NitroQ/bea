import { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import type { AsrEngineDescriptor, AsrEngineId, ProviderConfig, SetupStatus, SetupTool } from './types';
import Icon, { type IconName } from './Icon';

type Props = {
  status: SetupStatus;
  provider: ProviderConfig;
  apiKey: string;
  step: number;
  onStep: (step: number) => void;
  onSelectEngine: (id: AsrEngineId) => void;
  onInstallEngine: (id: AsrEngineId) => Promise<void>;
  onImportEngine: (path: string) => Promise<void>;
  onRepairTools: (ids: Array<'ffmpeg' | 'tesseract'>) => Promise<void>;
  onProviderChange: (provider: ProviderConfig) => void;
  onApiKeyChange: (value: string) => void;
  onTestProvider: () => Promise<void>;
  onComplete: () => void;
  notice: string;
  engineProgress?: EngineProgress | null;
};

const steps = [
  { label: 'Essentials', hint: 'Tools', icon: 'settings' as IconName },
  { label: 'Transcription', hint: 'Choose an engine', icon: 'waveform' as IconName },
  { label: 'AI provider', hint: 'Connect securely', icon: 'spark' as IconName },
  { label: 'Ready', hint: 'Start working', icon: 'check' as IconName },
];

function ToolRow({ tool, onRepair }: { tool: SetupTool; onRepair: () => void }) {
  const ready = tool.status === 'ready';
  return <div className="setup-row">
    <div className={`setup-status ${ready ? 'is-ready' : tool.status === 'repairing' ? 'is-working' : ''}`}><Icon name={ready ? 'check' : tool.status === 'repairing' ? 'refresh' : 'download'} size={15} /></div>
    <div className="setup-row-copy"><strong>{tool.name}</strong><span>{tool.description}</span></div>
    <div className="setup-row-state"><span className={`state-label ${ready ? 'ready' : 'attention'}`}>{ready ? 'Ready' : tool.status === 'repairing' ? 'Installing…' : 'Needs setup'}</span><small>{tool.detail}</small></div>
    {!ready && <button className="text-button" onClick={onRepair}>{tool.status === 'repairing' ? 'Working…' : 'Repair'}</button>}
  </div>;
}

export type EngineProgress = { id: string; downloaded: number; total: number | null; phase: 'downloading' | 'installing' };

function EngineCard({ engine, selected, progress, onSelect, onInstall }: { engine: AsrEngineDescriptor; selected: boolean; progress?: EngineProgress; onSelect: () => void; onInstall: () => void }) {
  const busy = progress !== undefined;
  const percent = progress?.total ? Math.min(100, Math.round((progress.downloaded / progress.total) * 100)) : null;
  return <button className={`engine-card ${selected ? 'selected' : ''} ${busy ? 'busy' : ''}`} onClick={() => { onSelect(); if (engine.status !== 'ready' && !busy) onInstall(); }} disabled={busy} type="button">
    <div className="engine-card-top"><span className="engine-icon"><Icon name={busy ? 'refresh' : 'waveform'} size={17} /></span>{engine.recommended && <span className="engine-recommended">Recommended</span>}<span className={`state-label ${engine.status === 'ready' ? 'ready' : busy ? 'working' : 'attention'}`}>{engine.status === 'ready' ? 'Installed' : progress?.phase === 'installing' ? 'Installing…' : busy ? 'Downloading…' : 'Available'}</span></div>
    <strong>{engine.name}</strong><p>{engine.description}</p>
    <div className="engine-meta"><span>{engine.languages}</span><span>{engine.size}</span></div>
    {busy && <div className="engine-progress"><div className="engine-progress-track"><span style={{ width: percent === null ? '100%' : `${percent}%` }} className={percent === null ? 'indeterminate' : undefined} /></div><small>{progress.phase === 'installing' ? 'Verifying and extracting…' : percent === null ? `${formatBytes(progress.downloaded)} downloaded` : `${percent}% · ${formatBytes(progress.downloaded)} of ${formatBytes(progress.total ?? 0)}`}</small></div>}
    <div className="engine-card-bottom"><span>{engine.detail}</span>{engine.status !== 'ready' && !busy && <span className="text-button">Install <Icon name="arrow-right" size={13} /></span>}</div>
    {selected && <span className="engine-selected"><Icon name="check" size={14} /></span>}
  </button>;
}

function formatBytes(bytes: number) {
  if (bytes < 1_000_000) return `${Math.round(bytes / 1000)} KB`;
  return `${(bytes / 1_000_000).toFixed(1)} GB`;
}

export default function SetupFlow({ status, provider, apiKey, step, onStep, onSelectEngine, onInstallEngine, onImportEngine, onRepairTools, onProviderChange, onApiKeyChange, onTestProvider, onComplete, notice, engineProgress }: Props) {
  const [openRouterModels, setOpenRouterModels] = useState<{ id: string; name: string }[]>([]);
  useEffect(() => {
    invoke<{ id: string; name: string }[]>('fetch_openrouter_models_command')
      .then(setOpenRouterModels)
      .catch(() => setOpenRouterModels([])); // offline: the input still accepts a typed id
  }, []);
  const [importing, setImporting] = useState(false);
  const selectedEngine = status.engines.find((engine) => engine.id === status.selectedEngine) ?? status.engines[0];
  const requiredToolsReady = status.tools.every((tool) => tool.status === 'ready');
  const engineReady = selectedEngine?.status === 'ready';
  const canContinue = step === 0 ? requiredToolsReady : step === 1 ? engineReady : step === 2 ? status.provider.verified : true;
  const providerLabel = provider.kind === 'OpenRouter' ? 'OpenRouter' : provider.kind === 'ClaudeCompatible' ? 'Claude-compatible proxy' : 'OpenAI-compatible';
  const selectedEngineSize = useMemo(() => selectedEngine?.size ?? '—', [selectedEngine]);

  async function choosePackage() {
    setImporting(true);
    try {
      const selected = await open({ multiple: false, directory: false, filters: [{ name: 'Verified Bea model package', extensions: ['bz2', 'zip', 'tar', 'onnx'] }] });
      const path = Array.isArray(selected) ? selected[0] : selected;
      if (path) await onImportEngine(path);
    } catch (error) {
      alert(`Unable to import the selected package: ${String(error)}`);
    } finally { setImporting(false); }
  }

  async function chooseFolder() {
    setImporting(true);
    try {
      const selected = await open({ multiple: false, directory: true });
      const path = Array.isArray(selected) ? selected[0] : selected;
      if (path) await onImportEngine(path);
    } catch (error) {
      alert(`Unable to import the selected folder: ${String(error)}`);
    } finally { setImporting(false); }
  }

  return <main className="setup-shell">
    <aside className="setup-rail">
      <div className="brand-mark"><span>bea</span><small>meeting assistant</small></div>
      <div className="setup-rail-title">Set up Bea</div>
      <nav className="setup-steps" aria-label="Setup steps">{steps.map((item, index) => <button type="button" key={item.label} className={`setup-step ${step === index ? 'active' : ''} ${index < step ? 'complete' : ''}`} onClick={() => index <= step && onStep(index)}><span className="step-number">{index < step ? <Icon name="check" size={13} /> : index + 1}</span><span><strong>{item.label}</strong><small>{item.hint}</small></span><Icon name={item.icon} size={15} /></button>)}</nav>
      <div className="setup-rail-footer"><span className="local-dot" />Local-first by default<span className="setup-version">v0.1</span></div>
    </aside>
    <section className="setup-main">
      <div className="setup-topbar"><span>Initial setup</span><span className="setup-progress">Step {Math.min(step + 1, 4)} of 4</span></div>
      {step === 0 && <div className="setup-page"><div className="setup-heading"><span className="page-kicker">01 · Essentials</span><h1>Let’s get your workspace ready.</h1><p>Bea runs quietly on your computer. We’ll check the small pieces it needs before you create your first meeting.</p></div><div className="setup-section"><div className="setup-section-heading"><div><h2>Local tools</h2><p>Required for media conversion and slide OCR.</p></div><span className="section-count">{status.tools.filter((tool) => tool.status === 'ready').length} / {status.tools.length} ready</span></div>{status.tools.map((tool) => <ToolRow tool={tool} key={tool.id} onRepair={() => onRepairTools([tool.id])} />)}</div><div className="setup-note"><Icon name="spark" size={15} /><span>Nothing leaves your computer during setup. Provider keys are only used for a connection test and stored outside the project database.</span></div></div>}
      {step === 1 && <div className="setup-page"><div className="setup-heading"><span className="page-kicker">02 · Transcription</span><h1>Choose how Bea listens.</h1><p>Pick one local engine for this computer. You can add another later in Settings without changing existing meetings.</p></div><div className="engine-grid">{status.engines.map((engine) => <EngineCard key={engine.id} engine={engine} selected={engine.id === status.selectedEngine} progress={engineProgress?.id === engine.id ? engineProgress : undefined} onSelect={() => onSelectEngine(engine.id)} onInstall={() => void onInstallEngine(engine.id)} />)}</div><button className="import-model" type="button" onClick={() => void choosePackage()} disabled={importing}><span><Icon name="upload" size={16} /></span><div><strong>{importing ? 'Checking package…' : 'Add from file'}</strong><small>Download failed or offline? Use a verified Bea manifest or supported sherpa-onnx package.</small></div><Icon name="arrow-right" size={15} /></button><button className="import-model secondary-import" type="button" onClick={() => void chooseFolder()} disabled={importing}><span><Icon name="folder" size={16} /></span><div><strong>Add extracted folder</strong><small>Choose a folder with the exact verified engine layout.</small></div><Icon name="arrow-right" size={15} /></button><div className="install-detail"><div><span className="detail-label">Selected engine</span><strong>{selectedEngine?.name ?? 'Choose an engine'}</strong></div><div><span className="detail-label">Download size</span><strong>{selectedEngineSize}</strong></div><div><span className="detail-label">Language coverage</span><strong>{selectedEngine?.languages ?? '—'}</strong></div></div></div>}
      {step === 2 && <div className="setup-page"><div className="setup-heading"><span className="page-kicker">03 · AI provider</span><h1>Connect the thinking layer.</h1><p>Use an OpenAI-compatible endpoint for summaries and minutes. Bea sends packed transcript context only after you confirm the preview.</p></div><div className="provider-layout"><div className="provider-form"><label>Provider<select value={provider.kind} onChange={(event) => onProviderChange({ ...provider, kind: event.target.value as ProviderConfig['kind'] })}><option value="OpenRouter">OpenRouter</option><option value="OpenAiCompatible">OpenAI-compatible</option><option value="ClaudeCompatible">Claude-compatible proxy</option></select></label><label>Minutes model
<input list={provider.kind === 'OpenRouter' ? 'openrouter-models' : undefined} value={provider.model} onChange={(event) => onProviderChange({ ...provider, model: event.target.value })} placeholder="Pick from the OpenRouter catalog" />
{provider.kind === 'OpenRouter' && <datalist id="openrouter-models">{openRouterModels.map((model) => <option key={model.id} value={model.id}>{model.name}</option>)}</datalist>}
<small className="field-help">{provider.kind === 'OpenRouter' ? 'Every OpenRouter model is available; the selected model generates the minutes.' : 'Type the model id, e.g. openai/gpt-4o-mini.'}</small></label><label>Base URL<input value={provider.base_url} onChange={(event) => onProviderChange({ ...provider, base_url: event.target.value })} placeholder="https://…/v1" /></label><label>API key<input type="password" value={apiKey} onChange={(event) => onApiKeyChange(event.target.value)} placeholder="Paste a key for the connection test" autoComplete="off" /><small className="field-help">Stored securely in Windows Credential Manager. Bea never writes the key to SQLite.</small></label><button className="verify-button" type="button" onClick={() => void onTestProvider()} disabled={!provider.model || !provider.base_url || !apiKey}><Icon name={status.provider.verified ? 'check' : 'spark'} size={15} />{status.provider.verified ? 'Connection verified' : 'Test connection'}<Icon name="arrow-right" size={14} /></button>{status.provider.message && <p className={`provider-message ${status.provider.verified ? 'success' : 'error'}`}>{status.provider.message}</p>}</div><div className="provider-disclosure"><div className="disclosure-heading"><span className="disclosure-icon"><Icon name="spark" size={16} /></span><div><strong>What will be sent?</strong><small>A small, reviewable request</small></div></div><div className="disclosure-list"><span><Icon name="check" size={14} />Transcript text and timestamps</span><span><Icon name="check" size={14} />Decisions and action-item context</span><span className="not-sent"><Icon name="x" size={14} />Raw audio or video</span><span className="not-sent"><Icon name="x" size={14} />Unselected evidence</span></div><div className="provider-endpoint"><span className="local-dot" />{providerLabel}<small>{provider.base_url || 'Endpoint not set'}</small></div></div></div></div>}
      {step === 3 && <div className="setup-page setup-ready"><div className="ready-illustration"><span><Icon name="check" size={28} /></span></div><div className="setup-heading"><span className="page-kicker">04 · Ready</span><h1>Your workspace is ready.</h1><p>Bea has everything it needs to keep meetings local, searchable, and easy to review.</p></div><div className="readiness-list"><div><Icon name="check" size={16} /><span><strong>Local media tools</strong><small>FFmpeg and Tesseract are ready</small></span></div><div><Icon name="check" size={16} /><span><strong>{selectedEngine?.name}</strong><small>{selectedEngine?.languages} transcription selected</small></span></div><div><Icon name="check" size={16} /><span><strong>{providerLabel}</strong><small>{provider.model || 'Provider verified'}</small></span></div></div><button className="primary large" type="button" onClick={onComplete}>Open meeting library <Icon name="arrow-right" size={16} /></button></div>}
      <div className="setup-footer"><span>{notice}</span><div>{step > 0 && <button className="secondary" type="button" onClick={() => onStep(step - 1)}><Icon name="arrow-left" size={14} />Back</button>}{step < 3 ? <button className="primary" type="button" disabled={!canContinue} onClick={() => onStep(step + 1)}>Continue <Icon name="arrow-right" size={14} /></button> : <span className="setup-footnote">You can revisit these choices in Settings.</span>}</div></div>
    </section>
  </main>;
}
