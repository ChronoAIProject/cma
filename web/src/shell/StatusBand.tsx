import { DISPLAY_ZONE_LABEL } from '../sessions/sessionTime';
import { BUILD } from './build';
import './StatusBand.css';

/**
 * The page's footer — docs/Web_Style.md §6.
 *
 * 40px, on every page, always reading in one order: the display zone, then
 * what this page is showing, then the build. The order is fixed so somebody
 * glancing at the bottom of the window finds the same fact in the same place
 * whatever they are looking at.
 *
 * In a multi-column view its 40px is shared by the columns' own footers, and
 * the rule still runs across. That is why this is one band divided rather than
 * a footer per column: two footers that are the same row cannot drift apart,
 * and §5's whole argument for setting a band's height is that accumulated
 * padding is what lets aligned columns drift.
 *
 * The zone is stated rather than assumed (§12): a fixed zone means the same
 * session reads the same to everyone who sees it, and a reader who does not
 * know which zone that is cannot check a time against their own.
 */

interface StatusBandProps {
  /**
   * What this page is showing, in the page's own words.
   *
   * Supplied by the page because only the page knows. A band that guessed
   * would be describing whichever surface it was written against.
   */
  readonly showing: string;
  /**
   * Anything the page must say about what it is NOT showing.
   *
   * §13 requires the canvas to state how many sessions are unattached and why,
   * because silent truncation reads as breakage.
   */
  readonly note?: string;
  /**
   * The width of the page's leading column, when it has one.
   *
   * The zone sits in that cell, so the band's first vertical rule lands on the
   * rail's, and the page reads as one surface rather than two. It follows the
   * column's width, because a collapsed rail is still a column.
   */
  readonly leadingColumn?: string;
}

export function StatusBand({ showing, note, leadingColumn }: StatusBandProps) {
  return (
    <footer className="band band--foot statusband">
      {/*
       * The leading column's share of the band. It is empty because the rail
       * has nothing to report at its foot — what it carries is a list, and the
       * list's own summary belongs in the list. The cell exists so the band's
       * first vertical rule continues the rail's, which is what makes the page
       * read as one surface rather than two.
       *
       * Nothing that must be read is put in it. A collapsed rail is 48px wide,
       * and a fact that disappears when a panel collapses is a fact collapsing
       * cost somebody — which §7 forbids.
       */}
      {leadingColumn !== undefined ? (
        <div
          className="statusband__cell statusband__lead"
          style={{ width: leadingColumn }}
          aria-hidden="true"
        />
      ) : null}

      <div className="statusband__cell statusband__main">
        <span className="band__label">Zone</span>
        <span className="band__value">{DISPLAY_ZONE_LABEL}</span>

        <span className="band__value statusband__showing">{showing}</span>
        {note !== undefined ? <span className="band__value statusband__note">{note}</span> : null}

        <span className="band__spacer" />
        <span className="band__label">Build</span>
        <span className="band__value">{BUILD}</span>
      </div>
    </footer>
  );
}
