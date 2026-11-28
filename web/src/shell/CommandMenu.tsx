import { useEffect, useId, useMemo, useState } from 'react';
import type { KeyboardEvent, ReactNode } from 'react';
import { Modal } from '../ui/Modal';
import { IconSearchOutline16 } from '../ui/icons';
import './CommandMenu.css';

/** One thing the menu can jump to or do. */
export interface Command {
  readonly id: string;
  readonly group: string;
  readonly label: string;
  /** A second line that tells similar labels apart, such as where an Agent lives. */
  readonly meta?: string;
  /** A trailing word such as a status. Searched as well as shown. */
  readonly hint?: string;
  /** Explicit lookup terms, such as canonical resource IDs, that are not displayed. */
  readonly searchTerms?: readonly string[];
  /** Lowercase queries that select this command first when they match in full. */
  readonly preferredQueries?: readonly string[];
  readonly icon?: ReactNode;
  readonly run: () => void;
}

/** Enough to find anything by typing; more would only be scrolled past. */
const RESULT_LIMIT = 60;

export function filterCommands(commands: readonly Command[], query: string): Command[] {
  const words = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return commands.slice(0, RESULT_LIMIT);
  const phrase = words.join(' ');
  const preferred = commands.find((command) => command.preferredQueries?.includes(phrase));
  const matched = commands.filter((command) => {
    if (command === preferred) return false;
    const text = [
      command.label,
      command.meta,
      command.hint,
      command.group,
      ...(command.searchTerms ?? []),
    ]
      .filter(Boolean)
      .join(' ')
      .toLowerCase();
    return words.every((word) => text.includes(word));
  });
  if (preferred) matched.unshift(preferred);
  return matched.slice(0, RESULT_LIMIT);
}

/**
 * The command menu — the shared style contractsand §15.
 *
 * It only ever shows what the shell has already read: opening it starts no
 * request and no work, so it can be summoned as often as somebody likes.
 * A combobox over a listbox, so focus stays in the field while the arrow keys
 * move the active option, which is what makes typing and choosing one motion.
 */
export function CommandMenu({
  open,
  onClose,
  commands,
}: {
  readonly open: boolean;
  readonly onClose: () => void;
  readonly commands: readonly Command[];
}) {
  return (
    <Modal open={open} onClose={onClose} title="Command menu" headless placement="command-menu">
      <CommandList commands={commands} onClose={onClose} />
    </Modal>
  );
}

function CommandList({
  commands,
  onClose,
}: {
  readonly commands: readonly Command[];
  readonly onClose: () => void;
}) {
  const id = useId();
  const [query, setQuery] = useState('');
  const [active, setActive] = useState(0);
  const results = useMemo(() => filterCommands(commands, query), [commands, query]);
  const groups = useMemo(() => {
    const ordered: { readonly name: string; readonly items: Command[] }[] = [];
    for (const command of results) {
      const group = ordered.find(({ name }) => name === command.group);
      if (group) group.items.push(command);
      else ordered.push({ name: command.group, items: [command] });
    }
    return ordered;
  }, [results]);
  // Grouping reorders results, so the active index follows the drawn order.
  const drawn = groups.flatMap(({ items }) => items);
  const activeIndex = Math.min(active, Math.max(0, drawn.length - 1));
  const activeCommand = drawn[activeIndex];

  useEffect(() => {
    document
      .getElementById(`${id}-option-${String(activeIndex)}`)
      ?.scrollIntoView?.({ block: 'nearest' });
  }, [id, activeIndex]);

  const choose = (command: Command | undefined) => {
    if (command === undefined) return;
    onClose();
    command.run();
  };

  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    // An IME uses these keys to compose; they are not ours until it finishes.
    if (event.nativeEvent.isComposing) return;
    const move = (to: number) => {
      event.preventDefault();
      if (drawn.length === 0) return;
      setActive((to + drawn.length) % drawn.length);
    };
    if (event.key === 'ArrowDown') move(activeIndex + 1);
    else if (event.key === 'ArrowUp') move(activeIndex - 1);
    else if (event.key === 'Home') move(0);
    else if (event.key === 'End') move(drawn.length - 1);
    else if (event.key === 'Enter') {
      event.preventDefault();
      choose(activeCommand);
    }
  };

  return (
    <div className="command-menu__body">
      <div className="command-menu__field">
        <IconSearchOutline16 />
        <input
          className="command-menu__input"
          role="combobox"
          aria-label="Search commands"
          aria-expanded="true"
          aria-controls={`${id}-list`}
          aria-autocomplete="list"
          aria-activedescendant={
            activeCommand === undefined ? undefined : `${id}-option-${String(activeIndex)}`
          }
          placeholder="Search agents, sandboxes, workspaces…"
          autoComplete="off"
          spellCheck={false}
          value={query}
          onChange={(event) => {
            setQuery(event.target.value);
            setActive(0);
          }}
          onKeyDown={onKeyDown}
        />
        <kbd className="command-menu__esc">Esc</kbd>
      </div>
      <div className="command-menu__list" id={`${id}-list`} role="listbox" aria-label="Results">
        {drawn.length === 0 ? (
          <p className="command-menu__empty" role="status">
            No results.
          </p>
        ) : (
          groups.map((group, groupIndex) => (
            // Indexed ids: a group name such as "Go to" contains a space, and
            // aria-labelledby reads a space as a separator between two ids.
            <div
              key={group.name}
              role="group"
              aria-labelledby={`${id}-group-${String(groupIndex)}`}
            >
              <div
                className="command-menu__group"
                id={`${id}-group-${String(groupIndex)}`}
                role="presentation"
              >
                {group.name}
              </div>
              {group.items.map((command) => {
                const at = drawn.indexOf(command);
                return (
                  <div
                    key={command.id}
                    id={`${id}-option-${String(at)}`}
                    className="command-menu__option"
                    role="option"
                    aria-selected={at === activeIndex}
                    onPointerMove={() => {
                      if (at !== activeIndex) setActive(at);
                    }}
                    onClick={() => choose(command)}
                  >
                    {command.icon === undefined ? null : (
                      <span className="command-menu__icon" aria-hidden="true">
                        {command.icon}
                      </span>
                    )}
                    <span className="command-menu__text">
                      <span className="command-menu__label">{command.label}</span>
                      {command.meta === undefined ? null : (
                        <span className="command-menu__meta">{command.meta}</span>
                      )}
                    </span>
                    {command.hint === undefined ? null : (
                      <span className="command-menu__hint">{command.hint}</span>
                    )}
                  </div>
                );
              })}
            </div>
          ))
        )}
      </div>
    </div>
  );
}

/**
 * ⌘K on Apple keyboards, Ctrl+K elsewhere, toggles the menu from anywhere.
 *
 * Skipped while an input method is composing, where the same chord can belong
 * to the composition rather than to the page.
 */
export function useCommandShortcut(toggle: () => void, enabled = true) {
  useEffect(() => {
    if (!enabled) return;
    const onKeyDown = (event: globalThis.KeyboardEvent) => {
      if (event.isComposing || event.altKey || event.shiftKey) return;
      if (!(event.metaKey || event.ctrlKey) || event.key.toLowerCase() !== 'k') return;
      event.preventDefault();
      toggle();
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [toggle, enabled]);
}
