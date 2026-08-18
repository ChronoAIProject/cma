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
 * One band across the whole window, undivided. It no longer carries a cell
 * per column, because the columns no longer draw their own footers — the
 * band is the page's foot, not a sum of the columns'.
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
   * §13 requires the canvas to state how many sessions are unattached and
   * why, because silent truncation reads as breakage.
   */
  readonly note?: string;
  /**
   * Below 30rem — §16. "The footer keeps the zone and the count and drops
   * the build." The zone and what the page is showing are what somebody
   * needs to read a time and know where they are; the build is what they
   * quote in a report, and a phone is not where that happens.
   */
  readonly compact?: boolean;
}

export function StatusBand({ showing, note, compact = false }: StatusBandProps) {
  return (
    <footer className="band band--foot statusband">
      <span className="band__label">Zone</span>
      <span className="band__value">{DISPLAY_ZONE_LABEL}</span>

      <span className="band__value statusband__showing">{showing}</span>
      {note !== undefined ? <span className="band__value statusband__note">{note}</span> : null}

      <span className="band__spacer" />
      {compact ? null : (
        <>
          <span className="band__label">Build</span>
          <span className="band__value">{BUILD}</span>
        </>
      )}
    </footer>
  );
}
