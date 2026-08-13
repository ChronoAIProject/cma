import type { ReactNode } from 'react';
import type { Session } from './types';
import { abilitiesFor } from './availability';
import { labelFor } from './types';
import { fullTimestamp, fullTimestampOrNone } from './sessionTime';
import './SessionDetails.css';

/**
 * What is known about the open session.
 *
 * Collapsible because it is reference material: someone reads it when they
 * want to know which model is answering or why a session stopped, and the rest
 * of the time it is width the terminal should have.
 *
 * Collapsed to a rail rather than removed entirely, so the way back is where
 * the panel was.
 */

interface SessionDetailsProps {
  readonly session: Session;
  readonly open: boolean;
  readonly onToggle: () => void;
  /** Why the session ended, when it ended badly. */
  readonly ending?: string;
  /**
   * What can be done to this session.
   *
   * Here rather than in the top bar: these act on the session being described,
   * and a control that ends something belongs beside the thing it ends, not in
   * chrome that looks the same whichever session is open.
   */
  readonly actions?: ReactNode;
}

export function SessionDetails({ session, open, onToggle, ending, actions }: SessionDetailsProps) {
  const reason = abilitiesFor(session).reason;
  if (!open) {
    return (
      <button
        type="button"
        className="details__handle"
        onClick={onToggle}
        aria-expanded={false}
        aria-label="Show session details"
      >
        <span aria-hidden="true">‹</span>
      </button>
    );
  }

  return (
    <aside className="details" aria-label="Session details">
      <div className="details__head">
        <h2 className="details__title">Session</h2>
        <button
          type="button"
          className="details__collapse"
          onClick={onToggle}
          aria-expanded
          aria-label="Hide session details"
        >
          <span aria-hidden="true">›</span>
        </button>
      </div>

      <dl className="details__list">
        <div className="details__row">
          <dt>Started</dt>
          <dd>{fullTimestamp(session.createdAt)}</dd>
        </div>
        <div className="details__row">
          <dt>Agent</dt>
          <dd>{session.kernel}</dd>
        </div>
        <div className="details__row">
          <dt>State</dt>
          <dd>{labelFor(session.lifecycle)}</dd>
        </div>
        {/*
         * Only present once a session has been moved between runtimes. Shown
         * because the alternative is someone noticing a gap in their terminal
         * and having no way to find out what caused it.
         */}
        {session.runtimeVersion !== undefined ? (
          <div className="details__row">
            <dt>Runtime</dt>
            <dd>{session.runtimeVersion}</dd>
          </div>
        ) : null}
        {session.rotations !== undefined && session.rotations > 0 ? (
          <div className="details__row">
            <dt>Moved</dt>
            <dd>
              {session.rotations === 1 ? 'once' : `${session.rotations} times`}
              {session.rotatedAt !== undefined
                ? `, last on ${fullTimestampOrNone(session.rotatedAt)}`
                : ''}
            </dd>
          </div>
        ) : null}
        <div className="details__row">
          <dt>Identifier</dt>
          {/*
           * Shown because it is what support and logs refer to, and wrapped
           * rather than truncated: a partial identifier is not one anyone can
           * use.
           */}
          <dd className="details__id">{session.id}</dd>
        </div>
      </dl>

      {/*
       * The reason the session cannot be typed into, stated wherever it is
       * true rather than only when something has gone wrong. A terminal that
       * ignores keystrokes without saying why is the thing people report as
       * broken.
       */}
      {reason !== undefined ? <p className="details__state">{reason}</p> : null}

      {ending !== undefined && ending !== '' ? <p className="details__ending">{ending}</p> : null}

      {actions !== undefined ? <div className="details__actions">{actions}</div> : null}
    </aside>
  );
}
