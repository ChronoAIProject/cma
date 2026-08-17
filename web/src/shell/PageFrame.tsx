import type { ReactNode } from 'react';
import { StatusBand } from './StatusBand';
import { TopBar } from './TopBar';
import { useWidth } from './widths';
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
  /**
   * The width of the column at the page's leading edge, when it has one.
   *
   * The band divides to match, so its first vertical rule lands on that
   * column's and the page reads as one surface (§6). It follows the column
   * rather than being fixed, because a collapsed rail is still a column and a
   * band that kept the open width would leave the rule 264px adrift.
   */
  readonly leadingColumn?: string;
  readonly children: ReactNode;
}

export function PageFrame({
  views,
  account,
  showing,
  note,
  leadingColumn,
  children,
}: PageFrameProps) {
  /*
   * §16: below 30rem the app bar drops to 48px and gives the switch and the
   * command field to the drawer. The bar keeps the mark and the account
   * control, so the window's top edge still reads as the same product.
   *
   * The switch is not merely hidden — a view somebody can no longer reach is
   * worse than one they have to open a drawer for — so the page is handed it
   * back to place in the drawer it already draws.
   */
  const compact = useWidth() === 'compact';

  return (
    <div className="pageframe" data-compact={compact}>
      <TopBar views={compact ? undefined : views} compact={compact}>
        {account}
      </TopBar>
      <div className="pageframe__body">{children}</div>
      <StatusBand
        showing={showing}
        note={note}
        leadingColumn={leadingColumn}
        /* §16: the band keeps the zone and the count and drops the build. */
        compact={compact}
      />
    </div>
  );
}
