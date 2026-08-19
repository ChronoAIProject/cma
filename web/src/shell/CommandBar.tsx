import { useMemo, useRef, useState } from 'react';
import type { Session } from '../sessions/types';
import type { Arrangement } from '../sessions/arrangement';
import { lastActivity } from '../sessions/lastActivity';
import { tabLabel } from '../sessions/sessionTime';
import { setAppearance, type Appearance } from '../theme/appearance';
import { PlateScrim, usePlateFocus } from '../ui/plate';
import './CommandBar.css';

/**
 * The command bar — docs/Web_Style.md §6 and §7.
 *
 * The plate the bar's trigger opens, reaching the surfaces it can already
 * reach and no more: sessions by time and last-activity line, the two
 * arrangements, new session, and the appearance controls. Anything else it
 * would offer belongs to a section of the document that does not exist yet,
 * so it lists nothing it cannot do.
 *
 * The focus trap, Escape and the focus return are the shared plate machinery
 * in ui/plate.ts — written once, for this and the dialog alike.
 */

interface CommandBarProps {
  readonly sessions: readonly Session[];
  readonly onClose: () => void;
  readonly onSelectSession: (id: string) => void;
  readonly onArrange: (arrangement: Arrangement) => void;
  /** Absent when the window has no room for the canvas — §16. */
  readonly canvasAvailable?: boolean;
  readonly onNewSession: () => void;
}

interface Command {
  readonly id: string;
  readonly label: string;
  readonly detail?: string;
  readonly run: () => void;
}

const APPEARANCES: readonly { readonly value: Appearance; readonly label: string }[] = [
  { value: 'system', label: 'Appearance: follow the system' },
  { value: 'light', label: 'Appearance: light' },
  { value: 'dark', label: 'Appearance: dark' },
];

export function CommandBar({
  sessions,
  onClose,
  onSelectSession,
  onArrange,
  canvasAvailable = true,
  onNewSession,
}: CommandBarProps) {
  const panelRef = useRef<HTMLDivElement>(null);
  const [query, setQuery] = useState('');
  const [activeIndex, setActiveIndex] = useState(0);

  usePlateFocus(panelRef, onClose);

  const commands = useMemo<readonly Command[]>(() => {
    const all: Command[] = [
      {
        id: 'new-session',
        label: 'New session',
        run: () => {
          onClose();
          onNewSession();
        },
      },
      ...sessions.map((session) => ({
        id: `session:${session.id}`,
        label: `Session ${tabLabel(session.createdAt)}`,
        detail: lastActivity(session),
        run: () => {
          onClose();
          onSelectSession(session.id);
        },
      })),
      {
        id: 'arrange-list',
        label: 'Arrange sessions as a list',
        run: () => {
          onClose();
          onArrange('list');
        },
      },
      // Omitted, not disabled, when the window has no room: the bar lists
      // nothing it cannot do.
      ...(canvasAvailable
        ? [
            {
              id: 'arrange-canvas',
              label: 'Arrange sessions on the canvas',
              run: () => {
                onClose();
                onArrange('canvas');
              },
            },
          ]
        : []),
      ...APPEARANCES.map(({ value, label }) => ({
        id: `appearance:${value}`,
        label,
        run: () => {
          onClose();
          setAppearance(value);
        },
      })),
    ];
    return all;
  }, [sessions, canvasAvailable, onArrange, onClose, onNewSession, onSelectSession]);

  const matched = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (needle === '') {
      return commands;
    }
    return commands.filter((command) =>
      `${command.label} ${command.detail ?? ''}`.toLowerCase().includes(needle),
    );
  }, [commands, query]);

  const active = Math.min(activeIndex, Math.max(0, matched.length - 1));

  return (
    <PlateScrim onClose={onClose} className="commandbar__scrim">
      <div
        className="plate commandbar"
        role="dialog"
        aria-modal="true"
        aria-label="Commands"
        ref={panelRef}
      >
        <input
          className="commandbar__field"
          type="text"
          value={query}
          placeholder="Find a session, or type a command"
          aria-label="Find a session, or type a command"
          onChange={(event) => {
            setQuery(event.target.value);
            setActiveIndex(0);
          }}
          onKeyDown={(event) => {
            if (event.key === 'ArrowDown') {
              event.preventDefault();
              setActiveIndex((index) => Math.min(index + 1, matched.length - 1));
            } else if (event.key === 'ArrowUp') {
              event.preventDefault();
              setActiveIndex((index) => Math.max(index - 1, 0));
            } else if (event.key === 'Enter') {
              event.preventDefault();
              matched[active]?.run();
            }
          }}
        />
        {matched.length === 0 ? (
          <p className="commandbar__empty">
            Nothing matches. The bar reaches sessions, arrangements, new session and appearance.
          </p>
        ) : (
          <ul className="commandbar__list" role="listbox" aria-label="Commands">
            {matched.map((command, index) => (
              <li key={command.id} role="option" aria-selected={index === active}>
                <button
                  type="button"
                  className={`menu__item${index === active ? ' menu__item--active' : ''}`}
                  onClick={command.run}
                  onPointerEnter={() => {
                    setActiveIndex(index);
                  }}
                >
                  {command.label}
                  {command.detail === undefined ? null : (
                    <span className="menu__kind">{command.detail}</span>
                  )}
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </PlateScrim>
  );
}
