import type { ReactNode } from 'react';
import type { Session } from './types';
import { labelFor, stateDotFor, toneFor } from './types';
import { Pill } from '../ui/Pill';
import { StateDot } from '../ui/StateDot';
import { tabLabel } from './sessionTime';
import './SessionHeader.css';

/**
 * The head band of the column the session is drawn in — docs/Web_Style.md §5
 * and §12.
 *
 * 52px, like every other column header, so its rule lands on the rail's to the
 * pixel and the page reads as one surface rather than two panels beside each
 * other.
 *
 * It carries the band's subject — the session time, at `lg`, the single
 * exception §5 allows to one type size per band — the facts that identify
 * which session the surface beneath is showing, and the session's lifecycle
 * controls.
 *
 * The controls are here rather than in the details sheet because §12 puts them
 * "at the head of the terminal they act on, visible without opening
 * anything". In the sheet they were reachable only by opening a panel that is
 * otherwise reference material, so ending a session meant first opening
 * something nobody wanted to read.
 */

interface SessionHeaderProps {
  /** The session the surface beneath is showing, or none. */
  readonly session: Session | null;
  /**
   * Whether it is parked on an answerable approval — §18.3: the pill reads
   * "Waiting for you" rather than the state's own name, because the wording
   * of a refusal belongs to the reason it exists.
   */
  readonly waiting?: boolean;
  /** The lifecycle controls that act on it. */
  readonly actions?: ReactNode;
  /**
   * The control that opens the session list, when the list is a drawer.
   *
   * §16 takes the rail out of the layout below 60rem, which leaves nothing in
   * the layout to open it with. It goes at the head of the column the drawer
   * covers, which is where somebody looks for the list.
   */
  readonly listHandle?: ReactNode;
}

export function SessionHeader({
  session,
  waiting = false,
  actions,
  listHandle,
}: SessionHeaderProps) {
  if (session === null) {
    return (
      <div className="band band--head sessionheader">
        {listHandle}
        <span className="band__label">Session</span>
        <span className="band__value">None open</span>
      </div>
    );
  }

  const dot = stateDotFor(toneFor(session.lifecycle));

  return (
    <div className="band band--head sessionheader">
      {listHandle}
      <span className="band__subject">{tabLabel(session.createdAt)}</span>
      {/* §12: the status pill — the state as a dot and a word. */}
      {waiting ? (
        <Pill>
          <StateDot state="warning" />
          Waiting for you
        </Pill>
      ) : (
        <Pill>
          {dot === undefined ? null : <StateDot state={dot} />}
          {labelFor(session.lifecycle)}
        </Pill>
      )}
      {/* The kernel and the plane, in mono, because both are identifiers. */}
      <span className="band__label">{session.kernel}</span>
      {session.plane === undefined ? null : <span className="band__label">{session.plane}</span>}
      <span className="band__spacer" />
      {actions}
    </div>
  );
}
