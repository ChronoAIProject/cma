import { useState } from 'react';
import type { Session } from './types';
import { stateDotFor, toneFor } from './types';
import { StateDot } from '../ui/StateDot';
import { Menu, type MenuEntry } from '../ui/Menu';
import { IconChevronDownOutline14, IconEllipsisOutline16 } from '../ui/icons';
import type { Arrangement } from './arrangement';
import type { Stage } from './progress';
import { lastActivity } from './lastActivity';
import { fullTimestamp, sortSessions, tabLabel, type SortOrder } from './sessionTime';
import { GROUPS, groupSessions, loadCollapsed, storeCollapsed, type GroupId } from './groups';
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
  /**
   * Sessions waiting on their owner's sign-in, by id.
   *
   * `provisioning` on its own cannot tell "a runtime is being built" from
   * "somebody has to sign in somewhere else", and the row said the second was
   * the first — sending them to wait for a thing that was waiting for them.
   */
  readonly signingIn?: Readonly<Record<string, unknown>>;
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
  signingIn = {},
}: SessionRailProps) {
  const [optionsOpen, setOptionsOpen] = useState(false);
  const [actionsFor_, setActionsFor] = useState<string | null>(null);
  /*
   * Which groups are closed. Read once, then owned here: storage is where the
   * preference survives a reload, not where it lives while the rail is open.
   */
  const [collapsed, setCollapsed] = useState<ReadonlySet<GroupId>>(loadCollapsed);

  // Sorted once, across the whole rail, then split. Sorting inside each group
  // would be three orders that can disagree with the one control that sets it.
  const ordered = sortSessions(sessions, order);
  const grouped = groupSessions(ordered);

  const toggleGroup = (id: GroupId) => {
    setCollapsed((was) => {
      const next = new Set(was);
      if (!next.delete(id)) {
        next.add(id);
      }
      storeCollapsed(next);
      return next;
    });
  };

  /*
   * No filter. Choosing "Stopped" and collapsing everything but Paused are the
   * same gesture done twice, and keeping both let the list hold two states
   * that disagreed — a group expanded inside a filter that excluded it.
   */
  const options: MenuEntry[] = [
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
          placement="rail__optionsanchor"
          selectedIds={[order, arrangement]}
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
      {sessions.length === 0 ? <p className="rail__empty">No sessions yet.</p> : null}

      <div className="rail__groups">
        {GROUPS.map((group) => {
          const inGroup = grouped[group.id];
          const shut = collapsed.has(group.id);
          return (
            <section className="rail__group" key={group.id}>
              {/*
               * The count is on the header, so a closed group still says how
               * much is inside it — and an empty group draws its header with a
               * zero rather than vanishing. A group that disappears is one
               * somebody looks for, and the zero is the answer to "where did
               * it go".
               */}
              <button
                type="button"
                className="rail__grouphead"
                aria-expanded={!shut}
                /*
                 * Named rather than left to the label and the count sitting
                 * beside each other: the accessible name is their text
                 * concatenated, which read as "Paused1".
                 */
                aria-label={`${group.label}, ${String(inGroup.length)} ${
                  inGroup.length === 1 ? 'session' : 'sessions'
                }`}
                onClick={() => {
                  toggleGroup(group.id);
                }}
              >
                <IconChevronDownOutline14
                  className={`rail__groupmark${shut ? ' rail__groupmark--shut' : ''}`}
                />
                <span className="rail__groupname">{group.label}</span>
                <span className="rail__groupcount">{inGroup.length}</span>
              </button>

              {shut ? null : (
                <ul
                  className="rail__list"
                  role="tablist"
                  aria-label={group.label}
                  aria-orientation="vertical"
                >
                  {inGroup.map((session) => {
                    const active = session.id === activeId;
                    const dot = stateDotFor(toneFor(session.lifecycle));
                    const activity = lastActivity(
                      session,
                      stages[session.id],
                      session.id === waitingId,
                      session.id in signingIn,
                    );
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
                                  setActionsFor((open) =>
                                    open === session.id ? null : session.id,
                                  );
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
              )}
            </section>
          );
        })}
      </div>
    </div>
  );
}
