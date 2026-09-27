import { useEffect, useRef, useState } from 'react';
import { SLASH_COMMANDS, SLASH_HELP } from './chatActions';
import ModelSelect from './ModelSelect';
import Select from './Select';
import type { OpenRouterModelInfo } from './types';
import { REASONING_EFFORTS, normalizeReasoningEffort } from './types';
import Icon from './Icon';
import BeaAvatar from './BeaAvatar';
import Markdown from './chatMarkdown';
import { copyText } from './chatFormat';
import type { TableExportHandler } from './chatFormat';

export type ChatMessage = { id: string; role: 'user' | 'bea' | 'system'; text: string; chip?: string; createdAt?: string };
type Attachment = { id: string; name: string; dataUrl: string };
export type ChatExportFormat = 'markdown' | 'text';
type Props = {
  messages: ChatMessage[];
  busy: boolean;
  hasCustomFormat: boolean;
  meetingModel: string;
  modelOptions: string[];
  openRouterModels?: OpenRouterModelInfo[];
  /// Per-meeting reasoning override; empty string means "use the Settings default".
  meetingReasoning: string;
  onReasoningChange: (reasoning: string) => void;
  /// Whether the selected model accepts images (null = unknown → no warning).
  visionCapable?: boolean | null;
  onModelChange: (model: string, contextTokens?: number | null) => void;
  onSend: (input: string, images: string[]) => void;
  onOpenCustomFormat: () => void;
  /// Re-asks the last question; hidden while a reply is already in flight.
  onRegenerate?: () => void;
  onExportTable?: TableExportHandler;
  onExportChat?: (format: ChatExportFormat) => void;
};

/// Chat clock label. `created_at` arrives from SQLite as an ISO string, so a
/// bad value has to degrade to an empty label rather than "Invalid Date".
export function chatTimeLabel(iso: string | undefined): string {
  if (!iso) return '';
  const parsed = new Date(iso);
  if (Number.isNaN(parsed.getTime())) return '';
  return parsed.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
}

/// True when the model id looks vision-capable and the provider catalog has no
/// entry for it (Codex / locally discovered lists). Mirrors the backend
/// heuristics so the UI can warn before a wasted send.
export function modelLooksVisionCapable(modelId: string, catalog: OpenRouterModelInfo[]): boolean {
  const entry = catalog.find((model) => model.id === modelId);
  if (entry) return entry.vision_capable;
  const id = modelId.toLowerCase();
  if (!id) return true; // provider default — unknown, stay quiet
  return ['gpt-4o', 'gpt-4.1', 'gpt-5', 'o3', 'o4', 'claude-3', 'claude-4', 'gemini', 'pixtral', 'llava', 'vl', 'vision'].some((marker) => id.includes(marker));
}

export default function ChatPanel({ messages, busy, hasCustomFormat, meetingModel, modelOptions, openRouterModels = [], meetingReasoning, onReasoningChange, visionCapable = null, onModelChange, onSend, onOpenCustomFormat, onRegenerate, onExportTable, onExportChat }: Props) {
  const [input, setInput] = useState('');
  const [menuOpen, setMenuOpen] = useState(false);
  const [exportOpen, setExportOpen] = useState(false);
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const [copiedId, setCopiedId] = useState<string | null>(null);
  // Sticky when the reader has scrolled up, so an incoming answer no longer
  // yanks them back to the bottom mid-paragraph.
  const [stickToBottom, setStickToBottom] = useState(true);
  const logRef = useRef<HTMLDivElement | null>(null);
  const inputRef = useRef<HTMLTextAreaElement | null>(null);
  const fileRef = useRef<HTMLInputElement | null>(null);

  const onLogScroll = () => {
    const log = logRef.current;
    if (!log) return;
    setStickToBottom(log.scrollHeight - log.scrollTop - log.clientHeight < 48);
  };
  useEffect(() => {
    if (stickToBottom && logRef.current) logRef.current.scrollTop = logRef.current.scrollHeight;
  }, [messages, busy, stickToBottom]);
  useEffect(() => {
    if (!copiedId) return;
    const timer = setTimeout(() => setCopiedId(null), 1600);
    return () => clearTimeout(timer);
  }, [copiedId]);

  // Grow with the content up to a ceiling, then scroll internally.
  useEffect(() => {
    const field = inputRef.current;
    if (!field) return;
    field.style.height = 'auto';
    field.style.height = `${Math.min(field.scrollHeight, 180)}px`;
  }, [input]);

  const jumpToLatest = () => {
    setStickToBottom(true);
    if (logRef.current) logRef.current.scrollTop = logRef.current.scrollHeight;
  };

  const addFiles = (files: Array<{ name: string; dataUrl: string }>) => {
    setAttachments((current) => [...current, ...files.map((file) => ({ id: crypto.randomUUID(), name: file.name, dataUrl: file.dataUrl }))].slice(0, 6));
  };
  const readAsDataUrl = (file: File): Promise<string> => new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.onerror = () => reject(reader.error);
    reader.readAsDataURL(file);
  });
  const ingestFiles = async (files: FileList | File[]) => {
    const images = Array.from(files).filter((file) => file.type.startsWith('image/'));
    for (const file of images.slice(0, 6)) {
      try { addFiles([{ name: file.name || 'image.png', dataUrl: await readAsDataUrl(file) }]); } catch { /* unreadable file: ignore */ }
    }
  };
  // Paste an image from the clipboard straight into the chat input.
  const onPaste = async (event: React.ClipboardEvent) => {
    const images = Array.from(event.clipboardData.items)
      .filter((item) => item.type.startsWith('image/'))
      .map((item) => item.getAsFile())
      .filter((file): file is File => Boolean(file));
    if (images.length) {
      event.preventDefault();
      await ingestFiles(images);
    }
  };

  const submit = () => {
    if (!input.trim() || busy) return;
    onSend(input.trim(), attachments.map((attachment) => attachment.dataUrl));
    setInput('');
    setAttachments([]);
    setMenuOpen(false);
  };
  // Open the command menu when the user types "/" at the start of the input.
  const onInputChange = (value: string) => {
    setInput(value);
    setMenuOpen(value.trimStart().startsWith('/') && !value.includes(' '));
  };
  const insertCommand = (command: string) => {
    setInput(`${command} `);
    setMenuOpen(false);
    inputRef.current?.focus();
  };
  const showVisionWarning = attachments.length > 0 && visionCapable === false;
  const lastBeaId = [...messages].reverse().find((message) => message.role === 'bea')?.id;
  const hasConversation = messages.some((message) => message.role !== 'system');
  return <div className="chat-panel" onPaste={onPaste}>
    <div className="chat-log" ref={logRef} onScroll={onLogScroll}>
      {messages.length === 0 && <div className="chat-empty"><div className="chat-empty-bea"><BeaAvatar variant="love" size={44} />
        <p>Ask about this meeting, or pick a command. You can also paste or attach a screenshot.</p></div>
        <ul>{SLASH_HELP.map((line) => <li key={line}><code>{line}</code></li>)}</ul>
      </div>}
      {messages.map((message) => {
        const time = chatTimeLabel(message.createdAt);
        const isLastBea = message.id === lastBeaId;
        return <div key={message.id} className={`chat-message ${message.role}`}>
          <div className="chat-message-body">
            {message.role === 'system' ? <span className="chat-plain">{message.text}</span> : <Markdown text={message.text} onExportTable={onExportTable} />}
            {message.chip && <span className="chat-frame-chip">{message.chip}</span>}
          </div>
          {(time || message.role !== 'system') && <div className="chat-message-foot">
            {time && <time className="chat-message-time">{time}</time>}
            {message.role !== 'system' && <div className="chat-message-actions">
              <button type="button" onClick={() => { void copyText(message.text).then((ok) => { if (ok) setCopiedId(message.id); }); }} title="Copy message" aria-label="Copy message">
                <Icon name={copiedId === message.id ? 'check' : 'copy'} size={12} />{copiedId === message.id ? 'Copied' : 'Copy'}
              </button>
              {onRegenerate && isLastBea && <button type="button" onClick={onRegenerate} disabled={busy} title="Ask again" aria-label="Regenerate answer">
                <Icon name="refresh" size={12} />Retry
              </button>}
            </div>}
          </div>}
        </div>;
      })}
      {busy && <div className="chat-message bea chat-busy">Thinking…</div>}
    </div>
    {!stickToBottom && messages.length > 0 && <button type="button" className="chat-jump-latest" onClick={jumpToLatest} aria-label="Jump to latest message">
      <Icon name="chevron-down" size={14} />Latest
    </button>}
    {attachments.length > 0 && <div className="chat-attachments" aria-label="Attached images">
      {attachments.map((attachment) => <figure key={attachment.id} className="chat-attachment">
        <img src={attachment.dataUrl} alt={attachment.name} />
        <button type="button" className="chat-attachment-remove" onClick={() => setAttachments((current) => current.filter((entry) => entry.id !== attachment.id))} aria-label={`Remove ${attachment.name}`}><Icon name="x" size={11} /></button>
      </figure>)}
      {showVisionWarning && <p className="chat-vision-warning"><Icon name="image" size={13} />This model may not read images directly — Bea will run local Tesseract OCR on {attachments.length === 1 ? 'it' : 'them'} instead, so layout/colors can be lost and text may be imperfect.</p>}
    </div>}
    <div className="chat-command-row">
      <div className="chat-model-select" title="Model for this meeting — leave empty to use the Settings default">
        <ModelSelect
          value={meetingModel}
          onChange={onModelChange}
          models={openRouterModels}
          ids={modelOptions}
          includeDefaultOption
          ariaLabel="Model for this meeting"
        />
      </div>
      <div className="chat-reasoning-select" title="Thinking effort for this meeting — leave on Default to use Settings">
        <Select
          value={meetingReasoning.trim() === '' ? '' : normalizeReasoningEffort(meetingReasoning)}
          options={[{ value: '', label: 'Reasoning: Default' }, ...REASONING_EFFORTS.map((option) => ({ value: option.id, label: `Reasoning: ${option.label}` }))]}
          onChange={onReasoningChange}
          ariaLabel="Reasoning effort for this meeting"
        />
      </div>
      <button type="button" className="secondary small" onClick={() => fileRef.current?.click()} title="Attach images for the model as reference">
        <Icon name="image" size={14} />Attach
      </button>
      <input ref={fileRef} type="file" accept="image/*" multiple hidden onChange={(event) => { if (event.target.files) void ingestFiles(event.target.files); event.target.value = ''; }} />
      <button type="button" className="secondary small" onClick={onOpenCustomFormat} title="Custom minutes format">
        <Icon name="file" size={14} />{hasCustomFormat ? 'Format set' : '/custom'}
      </button>
      <button type="button" className="secondary small" onClick={() => setMenuOpen((open) => !open)} title="Slash commands" aria-expanded={menuOpen}>
        <Icon name="chevron-down" size={14} />Commands
      </button>
      {menuOpen && <div className="chat-command-menu" role="menu">
        {SLASH_COMMANDS.map((command) => <button key={command.name} type="button" role="menuitem" onClick={() => insertCommand(command.name)}>
          <code>{command.name}</code><span>{command.description}</span>
        </button>)}
      </div>}
      {onExportChat && <div className="chat-export-menu">
        <button type="button" className="secondary small" onClick={() => setExportOpen((open) => !open)} title="Download this chat" aria-expanded={exportOpen} disabled={!hasConversation}>
          <Icon name="download" size={14} />Export
        </button>
        {exportOpen && <div className="chat-export-menu-list" role="menu">
          <button type="button" role="menuitem" onClick={() => { setExportOpen(false); onExportChat('markdown'); }}>
            <Icon name="file" size={14} /><span><strong>Markdown <small>.md</small></strong><small>Headings, tables and code kept as written</small></span>
          </button>
          <button type="button" role="menuitem" onClick={() => { setExportOpen(false); onExportChat('text'); }}>
            <Icon name="list" size={14} /><span><strong>Plain text <small>.txt</small></strong><small>Just the questions and answers</small></span>
          </button>
        </div>}
      </div>}
    </div>
    <form className="chat-input-row" onSubmit={(event) => { event.preventDefault(); submit(); }}>
      <textarea
        ref={inputRef}
        rows={1}
        value={input}
        onChange={(event) => onInputChange(event.target.value)}
        // Enter sends; Shift+Enter inserts a newline for multi-line questions.
        onKeyDown={(event) => {
          if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) {
            event.preventDefault();
            submit();
          }
        }}
        placeholder="Ask about the meeting, paste a screenshot, or /minutes /clarify /context /correction /custom"
        disabled={busy}
        aria-label="Meeting chat input"
      />
      <button type="submit" className="primary" disabled={busy || !input.trim()}><Icon name="arrow-right" size={14} />Send</button>
    </form>
  </div>;
}
