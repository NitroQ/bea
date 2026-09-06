import { useEffect, useRef, useState } from 'react';
import { SLASH_COMMANDS, SLASH_HELP } from './chatActions';
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
  const [menuOpen, setMenuOpen] = useState(false);
  const logRef = useRef<HTMLDivElement | null>(null);
  const inputRef = useRef<HTMLInputElement | null>(null);
  useEffect(() => { if (logRef.current) logRef.current.scrollTop = logRef.current.scrollHeight; }, [messages, busy]);
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
  return <div className="chat-panel">
    <div className="chat-log" ref={logRef}>
      {messages.length === 0 && <div className="chat-empty">
        <p>Ask about this meeting, or pick a command:</p>
        <ul>{SLASH_HELP.map((line) => <li key={line}><code>{line}</code></li>)}</ul>
      </div>}
      {messages.map((message) => <div key={message.id} className={`chat-message ${message.role}`}>{message.text}{message.chip && <span className="chat-frame-chip">{message.chip}</span>}</div>)}
      {busy && <div className="chat-message bea chat-busy">Thinking…</div>}
    </div>
    <div className="chat-command-row">
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
    </div>
    <form className="chat-input-row" onSubmit={(event) => { event.preventDefault(); if (!input.trim() || busy) return; onSend(input); setInput(''); setMenuOpen(false); }}>
      <input ref={inputRef} value={input} onChange={(event) => onInputChange(event.target.value)} placeholder="Ask about the meeting, or /clarify /context /correction /custom" disabled={busy} aria-label="Meeting chat input" />
      <button type="submit" className="primary" disabled={busy || !input.trim()}><Icon name="arrow-right" size={14} />Send</button>
    </form>
  </div>;
}
