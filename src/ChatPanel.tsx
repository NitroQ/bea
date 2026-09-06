import { useEffect, useRef, useState } from 'react';
import { SLASH_HELP } from './chatActions';
import Icon from './Icon';

export type ChatMessage = { id: string; role: 'user' | 'bea' | 'system'; text: string; chip?: string };
type Props = {
  messages: ChatMessage[];
  busy: boolean;
  hasCustomFormat: boolean;
  onSend: (input: string) => void;
  onOpenCustomFormat: () => void;
};

export default function ChatPanel({ messages, busy, hasCustomFormat, onSend, onOpenCustomFormat }: Props) {
  const [input, setInput] = useState('');
  const logRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => { if (logRef.current) logRef.current.scrollTop = logRef.current.scrollHeight; }, [messages, busy]);
  return <div className="chat-panel">
    <div className="chat-log" ref={logRef}>
      {messages.length === 0 && <div className="chat-empty">
        <p>Ask about this meeting, or use:</p>
        <ul>{SLASH_HELP.map((line) => <li key={line}><code>{line}</code></li>)}</ul>
      </div>}
      {messages.map((message) => <div key={message.id} className={`chat-message ${message.role}`}>{message.text}{message.chip && <span className="chat-frame-chip">{message.chip}</span>}</div>)}
      {busy && <div className="chat-message bea chat-busy">Thinking…</div>}
    </div>
    <form className="chat-input-row" onSubmit={(event) => { event.preventDefault(); if (!input.trim() || busy) return; onSend(input); setInput(''); }}>
      <button type="button" className="secondary small" onClick={onOpenCustomFormat} title="Custom minutes format">
        <Icon name="file" size={14} />{hasCustomFormat ? 'Format set' : '/custom'}
      </button>
      <input value={input} onChange={(event) => setInput(event.target.value)} placeholder="Ask about the meeting, or /clarify /context /correction /custom" disabled={busy} aria-label="Meeting chat input" />
      <button type="submit" className="primary" disabled={busy || !input.trim()}><Icon name="arrow-right" size={14} />Send</button>
    </form>
  </div>;
}
