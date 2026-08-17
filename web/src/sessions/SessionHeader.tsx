import type { ReactNode } from 'react';
import type { Session } from './types';
import { labelFor } from './types';
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
  /** The lifecycle controls that act on it. */
  readonly actions?: ReactNode;
}

export function SessionHeader({ session, actions }: SessionHeaderProps) {
  if (session === null) {
    return (
      <div className="band band--head sessionheader">
        <span className="band__label">Session</span>
        <span className="band__value">None open</span>
      </div>
    );
  }

  return (
    <div className="band band--head sessionheader">
      <span className="band__subject">{tabLabel(session.createdAt)}</span>
      <span className="band__label">{session.kernel}</span>
      <span className="band__value">{labelFor(session.lifecycle)}</span>
      <span className="band__spacer" />
      {actions}
    </div>
  );
}
