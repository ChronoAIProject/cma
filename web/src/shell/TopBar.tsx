import type { ReactNode } from 'react';
import { Mark } from './Mark';
import './TopBar.css';
import { StateDot } from '../ui/StateDot';

/**
 * The app bar — docs/Web_Style.md §6.
 *
 * One bar, 60px, on every page, so the window's top edge never moves as
 * somebody switches view. It carries, in order: the mark and wordmark, the
 * view switch, the command trigger, the live count, and the account control —
 * and nothing else.
 *
 * It is not navigation and must never become it.
 */

interface TopBarProps {
  /** The view switch. Absent on a page that has no views to switch between. */
  readonly views?: ReactNode;
  /**
   * Opens the command bar plate (§7). The trigger is a real control and is
   * never drawn disabled — §6 calls a permanently dead field in the middle of
   * the bar the loudest "unfinished" signal the product had — so a page with
   * no command bar to open draws no trigger rather than a dead one.
   */
  readonly onCommand?: () => void;
  /**
   * How many sessions are live right now. Data for the bar's count; absent
   * when the page has no sessions to count, and the slot renders nothing.
   */
  readonly live?: number;
  /**
   * Below 30rem — §16. The bar drops to 52px and gives the switch and the
   * command trigger to the drawer, keeping the mark and the account control
   * so the window's top edge still reads as the same product.
   */
  readonly compact?: boolean;
  /** The account control, at the trailing edge. */
  readonly children?: ReactNode;
}

export function TopBar({ views, onCommand, live, compact = false, children }: TopBarProps) {
  return (
    <header className="topbar">
      <div className="topbar__brand">
        <Mark />
        <span className="topbar__name">chronobot</span>
      </div>

      {views}

      {/*
       * The command trigger, per §6: a field-shaped button reading its own
       * purpose, with the key that opens it beside the words. A button rather
       * than an input because it opens a plate — nothing is typed here.
       */}
      {compact || onCommand === undefined ? (
        <span className="band__spacer" />
      ) : (
        <div className="topbar__command">
          <button type="button" className="topbar__trigger" onClick={onCommand}>
            <span className="topbar__triggerlabel">Find a session, or type a command</span>
            <kbd className="topbar__triggerkey">⌘K</kbd>
          </button>
        </div>
      )}

      {live === undefined ? null : (
        <span className="tag topbar__live">
          <StateDot state="ongoing" />
          {live} live
        </span>
      )}

      <div className="topbar__actions">{children}</div>
    </header>
  );
}
