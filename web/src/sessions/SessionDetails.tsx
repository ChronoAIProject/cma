import type { ReactNode } from 'react';
import type { Session } from './types';
import { fullTimestamp } from './sessionTime';
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
          <dd>{session.lifecycle}</dd>
        </div>
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

      {ending !== undefined && ending !== '' ? <p className="details__ending">{ending}</p> : null}

      {actions !== undefined ? <div className="details__actions">{actions}</div> : null}
    </aside>
  );
}
