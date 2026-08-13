import { useState } from 'react';
import type { Session } from './types';
import './SessionActions.css';

/**
 * What can be done to the session being described.
 *
 * Plain buttons rather than a menu. These are the only actions a session has,
 * there are at most three of them, and hiding them behind a control that has
 * to be discovered means someone reading an error that says "revive it" has
 * nowhere obvious to go.
 *
 * They are ordered by how much they destroy: reconnecting changes nothing,
 * ending stops the sandbox but keeps the transcript, deleting is final. Only
 * the last asks twice.
 */

interface SessionActionsProps {
  readonly session: Session;
  readonly onReconnect: () => void;
  readonly onTerminate: () => void;
  readonly onDelete: () => void;
  readonly onStartAgain: () => void;
  readonly busy?: boolean;
}

export function SessionActions({
  session,
  onReconnect,
  onTerminate,
  onDelete,
  onStartAgain,
  busy = false,
}: SessionActionsProps) {
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const ended = session.lifecycle === 'terminated';

  return (
    <div className="actions">
      {ended ? (
        <button type="button" className="actions__button" disabled={busy} onClick={onStartAgain}>
          Start a new session like this
        </button>
      ) : (
        <>
          <button type="button" className="actions__button" disabled={busy} onClick={onReconnect}>
            Reconnect
            <span className="actions__hint">Reattach to this session&rsquo;s sandbox</span>
          </button>
          <button type="button" className="actions__button" disabled={busy} onClick={onTerminate}>
            End session
            <span className="actions__hint">Stops the sandbox, keeps the transcript</span>
          </button>
        </>
      )}

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
          disabled={busy}
          onClick={() => {
            // Asked twice because this is the only act that cannot be undone.
            setConfirmingDelete(true);
          }}
        >
          Delete session
          <span className="actions__hint">Removes the session and its history</span>
        </button>
      )}
    </div>
  );
}
