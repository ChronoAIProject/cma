import type { ReactNode } from 'react';
import { StatusBand } from './StatusBand';
import { TopBar } from './TopBar';
import './PageFrame.css';

/**
 * The one page frame — docs/Web_Style.md §6.
 *
 * One app bar and one status band, on every page, so the window's top and
 * bottom edges never move as somebody switches view. Composed here rather than
 * assembled by each page, because the guarantee is that there is exactly one
 * of each: a page that built its own could grow a second bar without anything
 * noticing.
 *
 * The frame fills the viewport and does not scroll (§5). Whatever it is handed
 * owns its own overflow.
 */

interface PageFrameProps {
  /** The view switch, when the page has views to switch between. */
  readonly views?: ReactNode;
  /** The account control, at the bar's trailing edge. */
  readonly account?: ReactNode;
  /** What this page is showing, for the status band. */
  readonly showing: string;
  /** What this page is not showing, when that is a fact worth stating. */
  readonly note?: string;
  readonly children: ReactNode;
}

export function PageFrame({ views, account, showing, note, children }: PageFrameProps) {
  return (
    <div className="pageframe">
      <TopBar views={views}>{account}</TopBar>
      <div className="pageframe__body">{children}</div>
      <StatusBand showing={showing} note={note} />
    </div>
  );
}
