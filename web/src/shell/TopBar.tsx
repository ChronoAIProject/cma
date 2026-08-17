import type { ReactNode } from 'react';
import { Mark } from './Mark';
import './TopBar.css';

/**
 * The one bar the product has.
 *
 * Present in both states — before signing in and after — so the page has a
 * constant identity rather than appearing to be two different products. It
 * carries the name and whatever the current state needs on the right, and
 * nothing else: this is not navigation, and the product is still one page.
 */

interface TopBarProps {
  /** Controls belonging to the current state, shown at the trailing edge. */
  readonly children?: ReactNode;
}

export function TopBar({ children }: TopBarProps) {
  return (
    <header className="topbar">
      <div className="topbar__brand">
        <Mark />
        <span className="topbar__name">chronobot</span>
      </div>
      <div className="topbar__actions">{children}</div>
    </header>
  );
}
