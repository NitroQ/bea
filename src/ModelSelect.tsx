import { useEffect, useMemo, useRef, useState } from 'react';
import type { OpenRouterModelInfo } from './types';
import Icon from './Icon';

export type ModelCapability = { id: string; label: string; title: string };

/// Derives the capability set for a model id. OpenRouter catalog entries carry
/// real input-modality flags; discovered/local lists have none (every flag
/// false), so those options fall back to no badge.
export function capabilitiesFor(modelId: string, info?: OpenRouterModelInfo): ModelCapability[] {
  const capabilities: ModelCapability[] = [];
  if (info?.vision_capable) capabilities.push({ id: 'vision', label: 'Visual', title: 'Accepts images (vision)' });
  if (info?.audio_capable) capabilities.push({ id: 'audio', label: 'Audio', title: 'Accepts audio input' });
  if (info?.file_capable) capabilities.push({ id: 'file', label: 'File', title: 'Accepts file input' });
  return capabilities;
}

const contextLabel = (context?: number | null) => {
  if (!context) return null;
  if (context >= 1_000_000) return `${(context / 1_000_000).toFixed(context % 1_000_000 === 0 ? 0 : 1)}M ctx`;
  if (context >= 1000) return `${Math.round(context / 1000)}K ctx`;
  return `${context} ctx`;
};

/// Closed-picker label. When the value is empty the picker is not "nothing
/// selected": with a default option it means "follow the Settings default",
/// so the trigger must say that instead of the misleading "Select a model".
export const modelSelectTriggerLabel = (value: string, includeDefaultOption: boolean): string =>
  value || (includeDefaultOption ? 'Default model (Settings)' : 'Select a model');

type Option = { id: string; info?: OpenRouterModelInfo };

type Props = {
  value: string;
  onChange: (model: string) => void;
  /// Catalog entries for metadata only (capability badges + context length);
  /// option ids come from `ids`/`extraIds` so non-OpenRouter providers never
  /// show OpenRouter catalog entries.
  models?: OpenRouterModelInfo[];
  /// Plain ids when no catalog is available (local discovery).
  ids?: string[];
  /// Extra pinned entries (e.g. the Codex static model list).
  extraIds?: string[];
  includeDefaultOption?: boolean;
  ariaLabel?: string;
};

/// Shared AI-model dropdown with a custom listbox: the closed state shows the
/// selected model with its Visual/Audio/File capability badges; the open state
/// lists every model with badges and context length. Settings and the Chat
/// panel both render this so the picker looks the same everywhere. Native
/// <option> elements cannot carry badges (text-only in Chromium), hence the
/// custom popup.
export default function ModelSelect({ value, onChange, models = [], ids = [], extraIds = [], includeDefaultOption = false, ariaLabel = 'AI model' }: Props) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [active, setActive] = useState(0);
  const rootRef = useRef<HTMLDivElement | null>(null);
  const searchRef = useRef<HTMLInputElement | null>(null);
  const optionsRef = useRef<HTMLDivElement | null>(null);

  const options = useMemo<Option[]>(() => {
    const seen = new Set<string>();
    const list: Option[] = [];
    const push = (id: string) => { if (!seen.has(id)) { seen.add(id); list.push({ id, info: models.find((model) => model.id === id) }); } };
    if (includeDefaultOption) push('');
    ids.forEach(push);
    extraIds.forEach(push);
    if (value && !seen.has(value)) push(value);
    return list;
  }, [models, ids, extraIds, includeDefaultOption, value]);

  const filtered = useMemo<Option[]>(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return options;
    return options.filter((option) => (option.id || 'default model (settings)').toLowerCase().includes(needle));
  }, [options, query]);
  const activeIndex = Math.min(active, filtered.length - 1);

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: MouseEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener('mousedown', onPointerDown);
    return () => document.removeEventListener('mousedown', onPointerDown);
  }, [open]);

  useEffect(() => { if (open) searchRef.current?.focus(); }, [open]);
  // Keep the highlighted option visible while navigating with the keyboard.
  useEffect(() => {
    if (!open) return;
    optionsRef.current?.querySelector('.model-select-option.active')?.scrollIntoView({ block: 'nearest' });
  }, [activeIndex, open]);

  const openList = () => {
    setQuery('');
    setActive(Math.max(0, options.findIndex((option) => option.id === value)));
    setOpen(true);
  };
  const select = (id: string) => { onChange(id); setOpen(false); };
  const selectedOption = options.find((option) => option.id === value) ?? options[0];
  const selectedBadges = capabilitiesFor(value, selectedOption?.info);
  const selectedContext = contextLabel(selectedOption?.info?.context_length);

  const onKeyDown = (event: React.KeyboardEvent, inSearch = false) => {
    if (!open) {
      if (event.key === 'ArrowDown' || event.key === 'Enter' || event.key === ' ') { event.preventDefault(); openList(); }
      return;
    }
    if (event.key === 'Escape') { event.preventDefault(); setOpen(false); }
    else if (event.key === 'ArrowDown') { event.preventDefault(); setActive(Math.min(filtered.length - 1, activeIndex + 1)); }
    else if (event.key === 'ArrowUp') { event.preventDefault(); setActive(Math.max(0, activeIndex - 1)); }
    else if (event.key === 'Enter' || (!inSearch && event.key === ' ')) { event.preventDefault(); const option = filtered[activeIndex]; if (option) select(option.id); }
  };

  return (
    <div className={`model-select ${open ? 'open' : ''}`} ref={rootRef}>
      <button type="button" className="model-select-trigger" aria-haspopup="listbox" aria-expanded={open} aria-label={ariaLabel} onClick={() => (open ? setOpen(false) : openList())} onKeyDown={onKeyDown}>
        <span className="model-select-value">
          <span className="model-select-id">{modelSelectTriggerLabel(value, includeDefaultOption)}</span>
          {selectedBadges.map((badge) => <span key={badge.id} className={`model-capability cap-${badge.id}`} title={badge.title}>{badge.label}</span>)}
          {selectedContext && <span className="model-select-ctx">{selectedContext}</span>}
        </span>
        <Icon name="chevron-down" size={14} />
      </button>
      {open && (
        <div className="model-select-list">
          <div className="model-select-search">
            <Icon name="search" size={12} />
            <input ref={searchRef} value={query} onChange={(event) => { setQuery(event.target.value); setActive(0); }} onKeyDown={(event) => onKeyDown(event, true)} placeholder="Search models…" aria-label="Search models" spellCheck={false} />
          </div>
          <div className="model-select-options" role="listbox" ref={optionsRef} tabIndex={-1} onKeyDown={onKeyDown}>
            {filtered.map((option, index) => {
              const badges = capabilitiesFor(option.id, option.info);
              const context = contextLabel(option.info?.context_length);
              return (
                <button
                  type="button"
                  key={option.id || '(default)'}
                  role="option"
                  aria-selected={option.id === value}
                  className={`model-select-option ${option.id === value ? 'selected' : ''} ${index === activeIndex ? 'active' : ''}`}
                  onMouseEnter={() => setActive(index)}
                  onClick={() => select(option.id)}
                >
                  <span className="model-select-option-id">{option.id || 'Default model (Settings)'}</span>
                  {badges.length > 0 && <span className="model-select-option-caps">{badges.map((badge) => <span key={badge.id} className={`model-capability cap-${badge.id}`} title={badge.title}>{badge.label}</span>)}</span>}
                  {context && <span className="model-select-ctx">{context}</span>}
                </button>
              );
            })}
            {filtered.length === 0 && <div className="model-select-empty">No models match “{query.trim()}”</div>}
          </div>
          {models.length > 0 && <div className="model-select-foot">Visual · Audio · File reflect the provider's input modalities</div>}
        </div>
      )}
    </div>
  );
}
