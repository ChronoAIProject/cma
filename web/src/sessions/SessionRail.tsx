import type { Session } from './types';
import { toneFor } from './types';
import {
  DISPLAY_ZONE_LABEL,
  fullTimestamp,
  sortSessions,
  tabLabel,
  type SortOrder,
} from './sessionTime';
import './SessionRail.css';

/**
 * The sessions someone has, down the left.
 *
 * Vertical rather than a horizontal strip because sessions accumulate: a row
 * of tabs runs out of width after a handful and starts truncating the labels,
 * while a column scrolls without any of them becoming unreadable.
 *
 * Labelled by creation time, which is what actually distinguishes one session
 * from another — an identifier distinguishes them too, but not in a way anyone
 * can hold in their head.
 */

interface SessionRailProps {
  readonly sessions: readonly Session[];
  readonly activeId: string | null;
  readonly order: SortOrder;
  readonly onSelect: (id: string) => void;
  readonly onCreate: () => void;
  readonly onReorder: (order: SortOrder) => void;
}

export function SessionRail({
  sessions,
  activeId,
  order,
  onSelect,
  onCreate,
  onReorder,
}: SessionRailProps) {
  const ordered = sortSessions(sessions, order);

  return (
    <div className="rail">
      <div className="rail__head">
        <button type="button" className="rail__new" onClick={onCreate}>
          <span aria-hidden="true">+</span> New session
        </button>
        <button
          type="button"
          className="rail__sort"
          onClick={() => {
            onReorder(order === 'newest' ? 'oldest' : 'newest');
          }}
          // The control says which way the list is ordered, not which way
          // pressing it would order the list — the second reads as a
          // description of the current state and is routinely misread.
          aria-label={`Sorted ${order} first. Change order.`}
        >
          {order === 'newest' ? 'Newest first' : 'Oldest first'}
        </button>
      </div>

      <ul className="rail__list" role="tablist" aria-label="Sessions" aria-orientation="vertical">
        {ordered.map((session) => {
          const active = session.id === activeId;
          return (
            <li key={session.id}>
              <button
                type="button"
                role="tab"
                aria-selected={active}
                className={`rail__tab${active ? ' rail__tab--active' : ''}`}
                title={fullTimestamp(session.createdAt)}
                onClick={() => {
                  onSelect(session.id);
                }}
              >
                <span
                  className={`rail__dot tone-bg-${toneFor(session.lifecycle)}`}
                  aria-hidden="true"
                />
                <span className="rail__time">{tabLabel(session.createdAt)}</span>
                <span className="rail__meta">
                  {session.kernel} · {session.lifecycle}
                </span>
              </button>
            </li>
          );
        })}
      </ul>

      {sessions.length === 0 ? (
        <p className="rail__empty">No sessions yet.</p>
      ) : (
        <p className="rail__zone">Times in {DISPLAY_ZONE_LABEL}</p>
      )}
    </div>
  );
}
