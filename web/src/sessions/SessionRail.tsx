import { useState } from 'react';
import type { Session } from './types';
import { stateDotFor, toneFor } from './types';
import { StateDot } from '../ui/StateDot';
import { Menu, type MenuEntry } from '../ui/Menu';
import { IconEllipsisOutline16 } from '../ui/icons';
import type { Arrangement } from './arrangement';
import type { Stage } from './progress';
import { lastActivity } from './lastActivity';
import { fullTimestamp, sortSessions, tabLabel, type SortOrder } from './sessionTime';
import './SessionRail.css';

/**
 * The sessions someone has — `docs/Web_Style.md` §10.
 *
 * Vertical rather than a horizontal strip because sessions accumulate: a row
 * of tabs runs out of width after a handful and starts truncating labels,
 * while a column scrolls without any of them becoming unreadable.
 *
 * Labelled by creation time, which is what actually distinguishes one session
 * from another — an identifier does too, but not in a way anyone can hold in
 * their head.
 *
 * **One control in its header, not four.** Filtering, ordering and arranging
 * were three separate affordances across two bands; they are three answers to
 * "what am I looking at", and they belong on one card. What that buys is the
 * header dropping to a single 36px row, which is the height the list gets.
 *
 * Collapsed, this renders nothing at all. The sidebar around it becomes a
 * rail of icons, and a summary drawn here as well would be a second, quieter
 * answer to a question the rail already answers.
 */

/** What the list is narrowed to. Local: a filter is about this look at it. */
type RailFilter = 'all' | 'live' | 'stopped';

function isLive(session: Session): boolean {
  return toneFor(session.lifecycle) === 'live';
}

interface SessionRailProps {
  readonly sessions: readonly Session[];
  readonly activeId: string | null;
  readonly order: SortOrder;
  readonly onSelect: (id: string) => void;
  readonly onReorder: (order: SortOrder) => void;
  /**
   * How the list is arranged.
   *
   * §1: the control for an arrangement lives with the list it rearranges. It
   * does not stop at the browser — a terminal declares its own measurements
   * to the runtime drawing into it, so putting every attached session in a
   * node asks each agent to redraw at that width, and output already wrapped
   * narrow stays wrapped.
   */
  readonly arrangement: Arrangement;
  readonly onArrange: (arrangement: Arrangement) => void;
  /** Per-row actions, drawn on the row the pointer is over. */
  readonly actionsFor?: (session: Session) => readonly MenuEntry[];
  readonly onAction?: (session: Session, id: string) => void;
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
  onReorder,
  arrangement,
  onArrange,
  actionsFor,
  onAction,
  stages = {},
  waitingId = null,
}: SessionRailProps) {
  const [filter, setFilter] = useState<RailFilter>('all');
  const [optionsOpen, setOptionsOpen] = useState(false);
  const [actionsFor_, setActionsFor] = useState<string | null>(null);

  const filtered =
    filter === 'all' ? sessions : sessions.filter((s) => (filter === 'live') === isLive(s));
  const ordered = sortSessions(filtered, order);
  const counts: Record<RailFilter, number> = {
    all: sessions.length,
    live: sessions.filter(isLive).length,
    stopped: sessions.filter((s) => !isLive(s)).length,
  };

  const options: MenuEntry[] = [
    { type: 'label', id: 'show', text: 'Show' },
    { id: 'all', label: `All ${String(counts.all)}` },
    { id: 'live', label: `Live ${String(counts.live)}` },
    { id: 'stopped', label: `Stopped ${String(counts.stopped)}` },
    { type: 'separator', id: 'sep-order' },
    { type: 'label', id: 'order', text: 'Order' },
    { id: 'newest', label: 'Newest first' },
    { id: 'oldest', label: 'Oldest first' },
    { type: 'separator', id: 'sep-arrange' },
    { type: 'label', id: 'arrange', text: 'Arrange' },
    { id: 'list', label: 'List' },
    { id: 'canvas', label: 'Canvas' },
  ];

  const chooseOption = (id: string) => {
    setOptionsOpen(false);
    if (id === 'all' || id === 'live' || id === 'stopped') {
      setFilter(id);
      return;
    }
    if (id === 'newest' || id === 'oldest') {
      onReorder(id);
      return;
    }
    if (id === 'list' || id === 'canvas') {
      onArrange(id);
    }
  };

  return (
    <div className="rail">
      <div className="rail__header">
        <span className="rail__label">Sessions</span>
        <Menu
          open={optionsOpen}
          label="View options"
          align="end"
          selectedIds={[filter, order, arrangement]}
          items={options}
          onSelect={chooseOption}
          onClose={() => {
            setOptionsOpen(false);
          }}
          anchor={
            <button
              type="button"
              className="rail__options"
              aria-haspopup="menu"
              aria-expanded={optionsOpen}
              aria-label="View options"
              onClick={() => {
                setOptionsOpen((was) => !was);
              }}
            >
              <IconEllipsisOutline16 />
            </button>
          }
        />
      </div>

      {/*
        Above the list, not after it. The list grows to fill the column, so a
        message that followed it landed at the bottom of the sidebar — as far
        from the header it belongs under as the layout allows.
      */}
      {sessions.length === 0 ? (
        <p className="rail__empty">No sessions yet.</p>
      ) : ordered.length === 0 ? (
        // An empty list says what would have been there: the filter narrowed
        // to nothing, and the sessions still exist.
        <p className="rail__empty">Nothing matches this filter.</p>
      ) : null}

      <ul className="rail__list" role="tablist" aria-label="Sessions" aria-orientation="vertical">
        {ordered.map((session) => {
          const active = session.id === activeId;
          const dot = stateDotFor(toneFor(session.lifecycle));
          const activity = lastActivity(session, stages[session.id], session.id === waitingId);
          const actions = actionsFor?.(session) ?? [];

          return (
            <li className="rail__row" key={session.id}>
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
                {/*
                  The slot is held whether or not there is a dot in it, so an
                  ended session's title starts where every other one does.
                */}
                <span className="row-item__dot">
                  {dot === undefined ? null : <StateDot state={dot} />}
                </span>
                <span className="row-item__subject">{tabLabel(session.createdAt)}</span>
                {/*
                 * What makes a list of times into a list of work. It never
                 * wraps: a row that grows with its longest sentence makes the
                 * list a different height every time it reloads.
                 */}
                <span className="row-item__activity">{activity}</span>
              </button>

              {actions.length === 0 ? null : (
                <Menu
                  open={actionsFor_ === session.id}
                  label={`Actions for ${tabLabel(session.createdAt)}`}
                  align="end"
                  portal
                  items={actions}
                  onSelect={(id) => {
                    setActionsFor(null);
                    onAction?.(session, id);
                  }}
                  onClose={() => {
                    setActionsFor(null);
                  }}
                  anchor={
                    <button
                      type="button"
                      className="row-item__more"
                      aria-haspopup="menu"
                      aria-expanded={actionsFor_ === session.id}
                      aria-label={`Actions for the session started ${tabLabel(session.createdAt)}`}
                      onClick={() => {
                        setActionsFor((open) => (open === session.id ? null : session.id));
                      }}
                    >
                      <IconEllipsisOutline16 />
                    </button>
                  }
                />
              )}
            </li>
          );
        })}
      </ul>
    </div>
  );
}
