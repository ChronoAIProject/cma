import { useState } from 'react';
import type { Session } from './types';
import { abilitiesFor } from './availability';
import './SessionActions.css';

/**
 * What can be done to the session being described.
 *
 * Plain buttons rather than a menu. These are the only actions a session has,
 * there are at most four of them, and hiding them behind a control that has to
 * be discovered means someone reading "revive it to carry on" has nowhere
 * obvious to go.
 *
 * Every action is always shown, and disabled when the session's state does not
 * allow it. Removing a control that does not currently apply would make the
 * panel change shape as the session moves, and leave someone hunting for a
 * button that was there a moment ago. A disabled button with a reason under it
 * answers the question instead.
 *
 * They are ordered by how much they destroy: reviving and reconnecting change
 * nothing, ending releases the sandbox but keeps the transcript, deleting is
 * final. Only the last asks twice.
 */

interface SessionActionsProps {
  readonly session: Session;
  readonly onReconnect: () => void;
  readonly onRevive: () => void;
  readonly onStop: () => void;
  readonly onDelete: () => void;
  readonly busy?: boolean;
}

export function SessionActions({
  session,
  onReconnect,
  onRevive,
  onStop,
  onDelete,
  busy = false,
}: SessionActionsProps) {
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const can = abilitiesFor(session);

  return (
    <div className="actions">
      {can.canRevive ? (
        <button
          type="button"
          className="actions__button actions__button--primary"
          disabled={busy}
          onClick={onRevive}
        >
          Revive session
          <span className="actions__hint">Start it again and carry on where you left off</span>
        </button>
      ) : (
        <button
          type="button"
          className="actions__button"
          disabled={busy || !can.canReconnect}
          onClick={onReconnect}
        >
          Reconnect
          <span className="actions__hint">
            {can.canReconnect
              ? 'Reattach to this session’s sandbox'
              : 'Available once the session is running'}
          </span>
        </button>
      )}

      <button
        type="button"
        className="actions__button"
        disabled={busy || !can.canStop}
        onClick={onStop}
      >
        End session
        <span className="actions__hint">
          {can.canStop
            ? 'Releases the sandbox, keeps the history'
            : 'This session is not running'}
        </span>
      </button>

      {confirmingDelete ? (
        <div className="actions__confirm">
          <p className="actions__question">Delete this session and its history?</p>
          <div className="actions__row">
            <button
              type="button"
              className="actions__button"
              onClick={() => {
                setConfirmingDelete(false);
              }}
            >
              Keep it
            </button>
            <button
              type="button"
              className="actions__button actions__button--danger"
              disabled={busy}
              onClick={() => {
                setConfirmingDelete(false);
                onDelete();
              }}
            >
              Delete permanently
            </button>
          </div>
        </div>
      ) : (
        <button
          type="button"
          className="actions__button actions__button--danger"
          disabled={busy || !can.canDelete}
          onClick={() => {
            // Asked twice because this is the only act that cannot be undone.
            setConfirmingDelete(true);
          }}
        >
          Delete session
          <span className="actions__hint">
            {can.canDelete
              ? 'Removes the session and its history'
              : 'Not while the session is being moved'}
          </span>
        </button>
      )}
    </div>
  );
}
