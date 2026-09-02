import type { ReactNode } from 'react';
import markMarkup from '../assets/brand/cma-mark-animated.svg?raw';
import type { CreationAbility } from '../sessions/availability';
import { BUILD } from './build';
import type { SurfaceMode } from './surfaceMode';
import { IconNewChatOutline16, IconPanelLeftOutline16, IconSearchOutline16 } from '../ui/icons';
import './Sidebar.css';

/**
 * The sidebar — `docs/Web_Style.md` §10.
 *
 * Everything the app bar and the status band used to hold, in the one column
 * that is always on screen. That is not a rearrangement for its own sake: a
 * bar and a band cost 100px of height on every page, and height is the axis a
 * terminal and a conversation both run out of first.
 *
 * Collapsed it is a 56px rail of 36px controls rather than nothing. A panel
 * that disappears takes its own way back with it, and the rail is what keeps
 * "show me the other sessions" reachable at any width.
 */

interface SidebarProps {
  readonly collapsed: boolean;
  readonly creation: CreationAbility;
  readonly surface: SurfaceMode;
  readonly chatAvailable: boolean;
  readonly onSurfaceChange: (surface: SurfaceMode) => void;
  readonly onToggleCollapsed: () => void;
  readonly onCreate: () => void;
  /** Opens the command plate. The rail expands the sidebar instead. */
  readonly onSearch: () => void;
  /** The session list, which takes whatever height is left. */
  readonly children: ReactNode;
  /**
   * What the list is showing, and what it is not.
   *
   * The status band's one irreplaceable fact: the canvas drops sessions past
   * its attachment budget, and truncation that says nothing reads as
   * breakage. It sits with the list rather than in a band because the list is
   * what it is about.
   */
  readonly status?: ReactNode;
  /** Whether operations is the view currently showing. */
  /** The account control. */
  readonly account: ReactNode;
}

export function Sidebar({
  collapsed,
  creation,
  surface,
  chatAvailable,
  onSurfaceChange,
  onToggleCollapsed,
  onCreate,
  onSearch,
  children,
  status,
  account,
}: SidebarProps) {
  const reasonId = 'sidebar-creation-reason';
  const describedBy = creation.reason === undefined ? undefined : reasonId;
  return (
    <div className={`sidebar${collapsed ? ' sidebar--collapsed' : ''}`}>
      <div className="sidebar__logorow">
        {/*
          The mark is a shortcut to a new session and carries no hover chrome:
          the pointer is the only affordance it needs, and a hover fill on a
          wordmark reads as the product name being a control.

          The animated mark mounts with this authenticated shell rather than
          with a session or view, so it draws once after sign-in and does not
          replay while somebody navigates their work. Its wrapper stays still.
        */}
        <button
          type="button"
          className="sidebar__brand"
          disabled={!creation.canCreate}
          aria-describedby={describedBy}
          onClick={onCreate}
        >
          <span
            className="sidebar__mark"
            aria-hidden="true"
            dangerouslySetInnerHTML={{ __html: markMarkup }}
          />
          {collapsed ? null : <span className="sidebar__name">CMA</span>}
        </button>
        <button
          type="button"
          className="sidebar__icon sidebar__toggle"
          aria-expanded={!collapsed}
          aria-label={collapsed ? 'Show the session list' : 'Hide the session list'}
          onClick={onToggleCollapsed}
        >
          <IconPanelLeftOutline16 />
        </button>
      </div>

      {collapsed ? null : (
        <span className="sidebar__build" title="The build this page was served from">
          {BUILD}
        </span>
      )}

      {!collapsed && chatAvailable ? (
        <div className="sidebar__surfaces" role="group" aria-label="Session surface">
          {(['chat', 'agent'] as const).map((candidate) => (
            <button
              key={candidate}
              type="button"
              aria-pressed={surface === candidate}
              onClick={() => {
                onSurfaceChange(candidate);
              }}
            >
              {candidate === 'chat' ? 'Chat' : 'Agent'}
            </button>
          ))}
        </div>
      ) : null}

      <button
        type="button"
        className="sidebar__new"
        disabled={!creation.canCreate}
        aria-describedby={describedBy}
        onClick={onCreate}
      >
        <IconNewChatOutline16 />
        {collapsed ? null : (
          <span className="sidebar__newlabel">
            {surface === 'chat' ? 'New Chat' : 'New session'}
          </span>
        )}
      </button>
      {creation.reason === undefined ? null : (
        <span className="control-reason" id={reasonId}>
          {creation.reason}
        </span>
      )}

      {/*
        Searching opens the command plate, closed or open. It used to expand
        the sidebar when closed, on the reasoning that results are a list and a
        list needs a column — but the plate carries its own list, over whatever
        is behind it, and a control that does something else depending on a
        state somewhere else is a control nobody can predict. Pressing search
        and being given a sidebar is not a slower way to search.
      */}
      <button
        type="button"
        className="sidebar__icon sidebar__search"
        aria-label="Find a session, or type a command"
        onClick={onSearch}
      >
        <IconSearchOutline16 />
        {collapsed ? null : (
          <>
            <span className="sidebar__searchlabel">Find a session, or type a command</span>
            <kbd className="sidebar__key">⌘K</kbd>
          </>
        )}
      </button>

      <div className="sidebar__list">{children}</div>

      {collapsed || status === undefined ? null : <p className="sidebar__status">{status}</p>}

      {/*
       * Operations moved into the account menu. It is one of §1's three views
       * and was the app bar's switch until §9 retired the bar; a row of its
       * own here made the foot a two-item navigation strip beside an account
       * control that omitted the one place an operator goes.
       *
       * The account control is rendered collapsed or not. It used to be
       * replaced by a static settings glyph when the sidebar was closed, which
       * left no way to sign out — invisible while the toggle did nothing, and
       * reachable the moment it worked.
       */}
      <div className="sidebar__foot">
        <div className="sidebar__account">{account}</div>
      </div>
    </div>
  );
}
