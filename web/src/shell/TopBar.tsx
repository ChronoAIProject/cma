import type { ReactNode } from 'react';
import { Mark } from './Mark';
import './TopBar.css';

/**
 * The app bar — docs/Web_Style.md §6.
 *
 * One bar, 56px, on every page, so the window's top edge never moves as
 * somebody switches view. It carries the mark, the view switch, the command
 * field and the account control, in that order, and nothing else.
 *
 * It is not navigation and must never become it.
 */

interface TopBarProps {
  /** The view switch. Absent on a page that has no views to switch between. */
  readonly views?: ReactNode;
  /** The account control, at the trailing edge. */
  readonly children?: ReactNode;
}

export function TopBar({ views, children }: TopBarProps) {
  return (
    <header className="topbar">
      <div className="topbar__brand">
        <Mark />
        <span className="topbar__name">chronobot</span>
      </div>

      {views}

      {/*
       * The command field, per §6.
       *
       * Disabled, and saying so, because what it DOES is not specified
       * anywhere: Web_Style.md names the field, calls the command bar a plate
       * and hands it to the drawer below 30rem, but gives it no key binding,
       * no contents, no results model and no empty state. §1 forbids
       * introducing an interaction rule in web/ that the document does not
       * carry, so the field is drawn in its place and refuses rather than
       * accepting text it would silently drop. §7: controls are disabled,
       * never removed, and every disabled control states why.
       */}
      <div className="topbar__command">
        <input
          className="field__control topbar__commandfield"
          type="text"
          disabled
          aria-label="Command"
          placeholder="Command — not yet available"
        />
      </div>

      <div className="topbar__actions">{children}</div>
    </header>
  );
}
