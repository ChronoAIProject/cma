import { useId, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { KeyboardEvent, RefObject } from 'react';
import { createPortal } from 'react-dom';
import { usePresence, presenceAttributes } from '../ui/Presence';
import {
  commandPrefix,
  commandQuery,
  matchingCommands,
  type SessionCommand,
} from './slashCommands';
import './ComposerCommands.css';

export function useComposerCommands(
  content: string,
  commands: readonly SessionCommand[] | undefined,
  enabled: boolean,
  input: RefObject<HTMLTextAreaElement | null>,
  replace: (value: string) => void,
) {
  const id = useId();
  const [dismissed, setDismissed] = useState<string>();
  const [selected, setSelected] = useState<string>();
  const query = commandQuery(content);
  const open = enabled && query !== undefined && dismissed !== content;
  const matches = useMemo(() => matchingCommands(commands ?? [], query ?? ''), [commands, query]);
  const current = matches.find((command) => command.name === selected) ?? matches[0];
  const activeIndex = current === undefined ? -1 : matches.indexOf(current);
  const choose = (command: SessionCommand) => {
    if (!open) return;
    replace(`${commandPrefix(command)} `);
    setSelected(undefined);
    input.current?.focus();
  };
  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (!open) return false;
    if (event.key === 'Escape') {
      setDismissed(content);
    } else if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      if (matches.length === 0) return false;
      const delta = event.key === 'ArrowDown' ? 1 : -1;
      setSelected(matches[(activeIndex + delta + matches.length) % matches.length]!.name);
    } else if ((event.key === 'Enter' || event.key === 'Tab') && !event.shiftKey && current) {
      choose(current);
    } else return false;
    event.preventDefault();
    event.stopPropagation();
    return true;
  };
  return {
    open,
    id,
    activeId: open && activeIndex >= 0 ? `${id}-${activeIndex}` : undefined,
    onKeyDown,
    onFocus: () => setDismissed(undefined),
    onBlur: () => setDismissed(content),
    panel: (
      <ComposerCommands
        id={id}
        input={input}
        open={open}
        matches={matches}
        activeIndex={activeIndex}
        reported={commands !== undefined}
        onChoose={choose}
      />
    ),
  };
}

function ComposerCommands({
  id,
  input,
  open,
  matches,
  activeIndex,
  reported,
  onChoose,
}: {
  readonly id: string;
  readonly input: RefObject<HTMLTextAreaElement | null>;
  readonly open: boolean;
  readonly matches: readonly SessionCommand[];
  readonly activeIndex: number;
  readonly reported: boolean;
  readonly onChoose: (command: SessionCommand) => void;
}) {
  const panel = useRef<HTMLDivElement>(null);
  const { present, exiting } = usePresence(open);
  useLayoutEffect(() => {
    if (!open || !present) return;
    const place = () => {
      const anchor = input.current?.closest('.agent-composer__editor');
      const element = panel.current;
      if (!anchor || !element) return;
      const rect = anchor.getBoundingClientRect();
      const viewport = window.visualViewport;
      const left = viewport?.offsetLeft ?? 0;
      const top = viewport?.offsetTop ?? 0;
      const width = viewport?.width ?? window.innerWidth;
      const panelWidth = Math.max(0, Math.min(rect.width, width - 24));
      element.style.left = `${Math.max(left + 12, Math.min(rect.left, left + width - panelWidth - 12))}px`;
      element.style.width = `${panelWidth}px`;
      element.style.maxHeight = `${Math.max(0, rect.top - top - 24)}px`;
      element.style.bottom = `${window.innerHeight - rect.top + 8}px`;
      element.style.visibility = 'visible';
    };
    place();
    const observer = new ResizeObserver(place);
    if (input.current) observer.observe(input.current);
    window.addEventListener('resize', place);
    window.addEventListener('scroll', place, true);
    window.visualViewport?.addEventListener('resize', place);
    window.visualViewport?.addEventListener('scroll', place);
    return () => {
      observer.disconnect();
      window.removeEventListener('resize', place);
      window.removeEventListener('scroll', place, true);
      window.visualViewport?.removeEventListener('resize', place);
      window.visualViewport?.removeEventListener('scroll', place);
    };
  }, [input, open, present]);
  useLayoutEffect(() => {
    if (open && activeIndex >= 0)
      document.getElementById(`${id}-${activeIndex}`)?.scrollIntoView?.({ block: 'nearest' });
  }, [id, activeIndex, matches, open]);
  if (!present) return null;
  return createPortal(
    <div
      ref={panel}
      id={id}
      role="listbox"
      aria-label="Agent commands"
      className="composer-commands"
      style={{ visibility: 'hidden' }}
      {...presenceAttributes(exiting)}
    >
      {matches.length === 0 ? (
        <p className="composer-commands__empty" role="status">
          {reported
            ? 'No matching commands.'
            : 'This session has not reported available commands yet.'}
        </p>
      ) : (
        matches.map((command, index) => (
          <button
            key={command.name}
            id={`${id}-${index}`}
            type="button"
            role="option"
            aria-selected={index === activeIndex}
            tabIndex={-1}
            className="composer-commands__item"
            onPointerDown={(event) => event.preventDefault()}
            onClick={() => {
              if (!exiting) onChoose(command);
            }}
          >
            <span className="composer-commands__name">{commandPrefix(command)}</span>{' '}
            <span className="composer-commands__description">{command.description}</span>
          </button>
        ))
      )}
    </div>,
    document.body,
  );
}
