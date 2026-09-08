import { useEffect, useId, useLayoutEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from 'react';
import Icon from './Icon';

export type SelectOption<T extends string> = { value: T; label: string };

type Props<T extends string> = {
  value: T;
  options: ReadonlyArray<SelectOption<T>>;
  onChange: (value: T) => void;
  className?: string;
  align?: 'start' | 'end';
  ariaLabel?: string;
};

export default function Select<T extends string>({ value, options, onChange, className, align = 'start', ariaLabel }: Props<T>) {
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  const [dropUp, setDropUp] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const popRef = useRef<HTMLDivElement>(null);
  const typeahead = useRef({ buffer: '', at: 0 });
  const listId = useId();

  const selectedIndex = Math.max(0, options.findIndex((option) => option.value === value));

  useEffect(() => {
    if (!open) return;
    function onPointerDown(event: PointerEvent) {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    }
    document.addEventListener('pointerdown', onPointerDown);
    return () => document.removeEventListener('pointerdown', onPointerDown);
  }, [open]);

  useLayoutEffect(() => {
    if (!open) return;
    const pop = popRef.current;
    const trigger = rootRef.current?.querySelector('.select-trigger');
    if (!pop || !trigger) return;
    const rect = trigger.getBoundingClientRect();
    const spaceBelow = window.innerHeight - rect.bottom;
    setDropUp(spaceBelow < pop.offsetHeight + 12 && rect.top > spaceBelow);
  }, [open]);

  useLayoutEffect(() => {
    if (!open) return;
    popRef.current?.querySelector<HTMLElement>(`[data-index="${active}"]`)?.scrollIntoView({ block: 'nearest' });
  }, [open, active]);

  function openList() {
    setActive(selectedIndex);
    setOpen(true);
  }

  function commit(index: number) {
    const option = options[index];
    if (option && option.value !== value) onChange(option.value);
    setOpen(false);
    rootRef.current?.querySelector<HTMLButtonElement>('.select-trigger')?.focus();
  }

  function onKeyDown(event: ReactKeyboardEvent) {
    if (!open) {
      if (event.key === 'ArrowDown' || event.key === 'ArrowUp' || event.key === 'Enter' || event.key === ' ') {
        event.preventDefault();
        openList();
      }
      return;
    }
    switch (event.key) {
      case 'ArrowDown': event.preventDefault(); setActive((current) => (current + 1) % options.length); break;
      case 'ArrowUp': event.preventDefault(); setActive((current) => (current - 1 + options.length) % options.length); break;
      case 'Home': event.preventDefault(); setActive(0); break;
      case 'End': event.preventDefault(); setActive(options.length - 1); break;
      case 'Enter': case ' ': event.preventDefault(); commit(active); break;
      case 'Escape': case 'Tab': setOpen(false); break;
      default: {
        if (event.key.length !== 1 || event.altKey || event.ctrlKey || event.metaKey) return;
        const now = Date.now();
        const state = typeahead.current;
        state.buffer = now - state.at > 500 ? event.key : state.buffer + event.key;
        state.at = now;
        const needle = state.buffer.toLowerCase();
        const index = options.findIndex((option) => option.label.toLowerCase().startsWith(needle));
        if (index !== -1) setActive(index);
      }
    }
  }

  return (
    <div
      ref={rootRef}
      className={`select-root${open ? ' open' : ''}${dropUp ? ' drop-up' : ''}${align === 'end' ? ' align-end' : ''}${className ? ` ${className}` : ''}`}
      onKeyDown={onKeyDown}
    >
      <button
        type="button"
        className="select-trigger"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? listId : undefined}
        aria-label={ariaLabel}
        onClick={() => (open ? setOpen(false) : openList())}
      >
        <span className="select-value">{options[selectedIndex]?.label ?? ''}</span>
        <Icon name="chevron-down" size={14} />
      </button>
      {open && (
        <div ref={popRef} id={listId} role="listbox" aria-activedescendant={`${listId}-${active}`} tabIndex={-1} className="select-pop">
          {options.map((option, index) => (
            <button
              key={option.value}
              type="button"
              id={`${listId}-${index}`}
              role="option"
              data-index={index}
              aria-selected={option.value === value}
              className={`select-option${index === active ? ' active' : ''}${option.value === value ? ' selected' : ''}`}
              onMouseEnter={() => setActive(index)}
              onClick={() => commit(index)}
            >
              <Icon name="check" size={13} />
              <span>{option.label}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
