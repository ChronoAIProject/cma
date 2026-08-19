import { useEffect, useRef, useState, type ReactNode } from 'react';
import type { Session } from './types';
import { stateDotFor, toneFor } from './types';
import { StateDot } from '../ui/StateDot';
import type { Stage } from './progress';
import { lastActivity } from './lastActivity';
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
 *
 * Collapsed it is a handle, never nothing (§7): a 48px strip that keeps both
 * bands, so the way back is exactly where the panel was and the page's rules
 * still line up. The strip carries the summary the list would have given —
 * the status dots in state order, and a count — because collapsing gives up
 * detail, not awareness.
 */

/** Which dots the collapsed strip shows, and in which order. */
const TONE_ORDER = ['live', 'wait', 'fail', 'ended'] as const;

/**
 * The filter pills — §12: `All 5`, `Live 2`, `Stopped 3`. Live is the live
 * tone; stopped is everything else still listed. Rail-local state, because a
 * filter is about this look at the list, not a setting to carry across tabs.
 */
type RailFilter = 'all' | 'live' | 'stopped';

function isLive(session: Session): boolean {
  return toneFor(session.lifecycle) === 'live';
}

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
  readonly collapsed: boolean;
  readonly onToggleCollapsed: () => void;
  /** The step each starting session has reached, for its activity line. */
  readonly stages?: Readonly<Record<string, Stage>>;
  /** The session parked on an answerable approval, when one is — §18.3. */
  readonly waitingId?: string | null;
}

export function SessionRail({
  sessions,
  activeId,
  order,
  onSelect,
  onCreate,
  onReorder,
  arrangement,
  collapsed,
  onToggleCollapsed,
  stages = {},
  waitingId = null,
}: SessionRailProps) {
  const [filter, setFilter] = useState<RailFilter>('all');
  const filtered =
    filter === 'all' ? sessions : sessions.filter((s) => (filter === 'live') === isLive(s));
  const ordered = sortSessions(filtered, order);
  const counts: Record<RailFilter, number> = {
    all: sessions.length,
    live: sessions.filter(isLive).length,
    stopped: sessions.filter((s) => !isLive(s)).length,
  };
  const handleRef = useRef<HTMLButtonElement>(null);
  // Focus moves to the handle when the panel collapses or opens, because §7
  // says so and because the alternative is stranding somebody on a control
  // that has just left the layout.
  const firstRender = useRef(true);

  useEffect(() => {
    if (firstRender.current) {
      firstRender.current = false;
      return;
    }
    handleRef.current?.focus();
  }, [collapsed]);

  // ⌃← collapses a column and ⌃→ opens it (§7). Bound on the document rather
  // than on the rail: the shortcut has to work while the caret is anywhere on
  // the page, which is the whole reason for having one.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (!event.ctrlKey || event.metaKey || event.altKey) {
        return;
      }
      const wants = event.key === 'ArrowLeft' ? true : event.key === 'ArrowRight' ? false : null;
      if (wants === null || wants === collapsed) {
        return;
      }
      event.preventDefault();
      onToggleCollapsed();
    };
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('keydown', onKeyDown);
    };
  }, [collapsed, onToggleCollapsed]);

  const handle = (
    <button
      ref={handleRef}
      type="button"
      className="collapse-handle rail__handle"
      aria-expanded={!collapsed}
      aria-label={collapsed ? 'Show the session list' : 'Hide the session list'}
      onClick={onToggleCollapsed}
    >
      {/*
       * The glyph points where the panel will go and reverses when collapsed,
       * so the control describes its outcome rather than its state (§7).
       */}
      <span aria-hidden="true">{collapsed ? '›' : '‹'}</span>
    </button>
  );

  if (collapsed) {
    const counted = TONE_ORDER.map((tone) => ({
      tone,
      count: sessions.filter((session) => toneFor(session.lifecycle) === tone).length,
    })).filter((entry) => entry.count > 0);

    return (
      <div className="rail rail--collapsed">
        <div className="band band--head rail__head">{handle}</div>
        <div className="band rail__chrome rail__chrome--collapsed">
          <span className="band__value">{sessions.length}</span>
        </div>
        <ul className="rail__summary" aria-label={`${String(sessions.length)} sessions`}>
          {counted.map(({ tone, count }) => (
            <li className="rail__summaryitem" key={tone}>
              {stateDotFor(tone) === undefined ? null : (
                <StateDot state={stateDotFor(tone)!} placement="rail__dot" />
              )}
              {/*
               * The number is drawn and the tone repeated in words, because a
               * summary carried by colour alone is not a summary (§10).
               */}
              <span className="rail__summarycount">{count}</span>
              <span className="sr-only">{tone}</span>
            </li>
          ))}
        </ul>
      </div>
    );
  }

  return (
    <div className="rail">
      <div className="band band--head rail__head">
        <span className="band__label">Sessions</span>
        <span className="band__value">{sessions.length}</span>
        <span className="band__spacer" />
        {arrangement}
        {handle}
      </div>

      {/*
       * The second band, stacked. §5 names this as the one documented
       * exception to the row shape: it sits below the head band, so the head
       * rule still runs straight across every column.
       */}
      <div className="band rail__chrome">
        {/* §12: the rail's one full-width primary key. */}
        <button
          type="button"
          className="button button--primary button--md rail__newsession"
          onClick={onCreate}
        >
          <span aria-hidden="true">+</span> New session
        </button>
        <div className="rail__filters">
          {(['all', 'live', 'stopped'] as const).map((option) => (
            <button
              key={option}
              type="button"
              className={`tag rail__filter${filter === option ? ' tag--on' : ''}`}
              aria-pressed={filter === option}
              onClick={() => {
                setFilter(option);
              }}
            >
              {option === 'all' ? 'All' : option === 'live' ? 'Live' : 'Stopped'} {counts[option]}
            </button>
          ))}
          <span className="band__spacer" />
          <button
            type="button"
            className="button button--ghost button--sm"
            onClick={() => {
              onReorder(order === 'newest' ? 'oldest' : 'newest');
            }}
            // The control says which way the list is ordered, not which way
            // pressing it would order the list — the second reads as a
            // description of the current state and is routinely misread.
            aria-label={`Sorted ${order} first. Change order.`}
          >
            {order === 'newest' ? 'Newest' : 'Oldest'}
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
                className="row-item"
                title={fullTimestamp(session.createdAt)}
                onClick={() => {
                  onSelect(session.id);
                }}
              >
                {stateDotFor(toneFor(session.lifecycle)) === undefined ? null : (
                  <StateDot
                    state={stateDotFor(toneFor(session.lifecycle))!}
                    placement="row-item__dot"
                  />
                )}
                <span className="row-item__subject">{tabLabel(session.createdAt)}</span>
                <span className="row-item__meta">{session.kernel}</span>
                {/*
                 * What makes a list of times into a list of work (§12). It
                 * never wraps: a row that grows with its longest sentence
                 * makes the list a different height every time it reloads.
                 */}
                <span className="row-item__activity">
                  {lastActivity(session, stages[session.id], session.id === waitingId)}
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
      {sessions.length === 0 ? (
        <p className="rail__empty">No sessions yet.</p>
      ) : ordered.length === 0 ? (
        // §14's rule, held here too: an empty list says what would have been
        // there. The filter narrowed to nothing; the sessions still exist.
        <p className="rail__empty">Nothing matches this filter.</p>
      ) : null}
    </div>
  );
}
