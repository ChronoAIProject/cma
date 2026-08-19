import type { ReactNode } from 'react';
import { Mark } from './Mark';
import { BUILD } from './build';
import {
  IconNewChatOutline16,
  IconPanelLeftOutline16,
  IconSearchOutline16,
  IconSettingsOutline16,
  IconDataOutline16,
} from '../ui/icons';
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
  /** Drawn only where the caller holds an operator capability. */
  readonly onOperations?: () => void;
  /** Whether operations is the view currently showing. */
  readonly operationsActive?: boolean;
  /** The account control. */
  readonly account: ReactNode;
}

export function Sidebar({
  collapsed,
  onToggleCollapsed,
  onCreate,
  onSearch,
  children,
  status,
  onOperations,
  operationsActive = false,
  account,
}: SidebarProps) {
  return (
    <div className={`sidebar${collapsed ? ' sidebar--collapsed' : ''}`}>
      <div className="sidebar__logorow">
        {/*
          The mark is a shortcut to a new session and carries no hover chrome:
          the pointer is the only affordance it needs, and a hover fill on a
          wordmark reads as the product name being a control.
        */}
        <button type="button" className="sidebar__brand" onClick={onCreate}>
          <Mark size={24} />
          {collapsed ? null : <span className="sidebar__name">chronobot</span>}
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

      <button type="button" className="sidebar__new" onClick={onCreate}>
        <IconNewChatOutline16 />
        {collapsed ? null : <span className="sidebar__newlabel">New session</span>}
      </button>

      {/*
        Collapsed, searching expands the sidebar rather than opening the plate
        over a rail: the results are a list, and a list needs the column the
        rail just gave up.
      */}
      <button
        type="button"
        className="sidebar__icon sidebar__search"
        aria-label="Find a session, or type a command"
        onClick={collapsed ? onToggleCollapsed : onSearch}
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

      <div className="sidebar__foot">
        {onOperations === undefined ? null : (
          <button
            type="button"
            className="sidebar__footrow"
            aria-label="Operations"
            /*
             * Pressed rather than a second row to go back: it is one place
             * that is either showing or not, and a separate "Sessions" row
             * would imply the sidebar itself was somewhere else.
             */
            aria-pressed={operationsActive}
            onClick={onOperations}
          >
            <IconDataOutline16 />
            {collapsed ? null : <span>Operations</span>}
          </button>
        )}
        <div className="sidebar__account" aria-label="Settings">
          {collapsed ? <IconSettingsOutline16 /> : account}
        </div>
      </div>
    </div>
  );
}
