import type { ReactNode } from 'react';
import type { Session } from './types';
import { toneFor } from './types';
import { fullTimestamp, sortSessions, tabLabel, type SortOrder } from './sessionTime';
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
 *
 * Two bands at its head, per docs/Web_Style.md §5. The first is the head band
 * every column shares, so its rule lands on the session header's. The rail
 * needs more chrome than fits there, and a column that needs more chrome adds
 * a band beneath the first rather than growing its header — the rail's second
 * band being the one documented exception to the row shape, since it stacks
 * the new-session key over the filters.
 */

interface SessionRailProps {
  readonly sessions: readonly Session[];
  readonly activeId: string | null;
  readonly order: SortOrder;
  readonly onSelect: (id: string) => void;
  readonly onCreate: () => void;
  readonly onReorder: (order: SortOrder) => void;
  /**
   * How the list is arranged, drawn in the rail's own head band.
   *
   * §1: the control for an arrangement lives with the list it rearranges. It
   * used to sit in the app bar, which made the bar differ between a page that
   * has a list and one that does not — and §6 requires one bar that never
   * changes shape as somebody switches view.
   */
  readonly arrangement?: ReactNode;
  /** The collapse handle, at the trailing edge of the head band. */
  readonly collapse?: ReactNode;
}

export function SessionRail({
  sessions,
  activeId,
  order,
  onSelect,
  onCreate,
  onReorder,
  arrangement,
  collapse,
}: SessionRailProps) {
  const ordered = sortSessions(sessions, order);

  return (
    <div className="rail">
      <div className="band band--head rail__head">
        <span className="band__label">Sessions</span>
        <span className="band__value">{sessions.length}</span>
        <span className="band__spacer" />
        {arrangement}
        {collapse}
      </div>

      {/*
       * The second band, stacked. §5 names this as the one documented
       * exception to the row shape: it sits below the head band, so the head
       * rule still runs straight across every column.
       */}
      <div className="band rail__chrome">
        <button
          type="button"
          className="button button--block press--quiet rail__new"
          onClick={onCreate}
        >
          <span aria-hidden="true">+</span> New session
        </button>
        <div className="rail__filters">
          <button
            type="button"
            className="button button--sm press--quiet"
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

      {/*
       * No footer here. §6 gives the page one 40px band and says the columns
       * share it, so the rail ends at that band's rule rather than drawing a
       * second one a few pixels above it.
       *
       * The zone is not restated either: §12 puts it in the status band, which
       * is on every page, so a rail that also carried it would be the second
       * place a reader had to reconcile.
       */}
      {sessions.length === 0 ? <p className="rail__empty">No sessions yet.</p> : null}
    </div>
  );
}
