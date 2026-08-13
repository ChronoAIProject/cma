import type { Session } from './types';
import { labelFor, toneFor } from './types';
import './SessionTabs.css';

/**
 * The session tab strip. A switcher, not navigation — the product is one page.
 *
 * Every tab pairs its status dot with the state word, because Web_Style.md
 * forbids conveying state through colour alone.
 */

interface SessionTabsProps {
  readonly sessions: readonly Session[];
  readonly activeId: string | null;
  readonly onSelect: (id: string) => void;
  readonly onCreate: () => void;
}

export function SessionTabs({ sessions, activeId, onSelect, onCreate }: SessionTabsProps) {
  return (
    <div className="tabs" role="tablist" aria-label="Sessions">
      <div className="tabs__strip">
        {sessions.map((session) => {
          const active = session.id === activeId;
          return (
            <button
              key={session.id}
              type="button"
              role="tab"
              aria-selected={active}
              className="tab"
              data-active={active}
              onClick={() => {
                onSelect(session.id);
              }}
            >
              <span
                className={`tab__dot tone-bg-${toneFor(session.lifecycle)}`}
                aria-hidden="true"
              />
              <span className="tab__id">{session.id}</span>
              <span className="tab__state">{labelFor(session.lifecycle)}</span>
            </button>
          );
        })}
      </div>

      <button type="button" className="tab tab--new" onClick={onCreate}>
        <span aria-hidden="true">+</span>
        <span className="sr-only">New session</span>
      </button>
    </div>
  );
}
