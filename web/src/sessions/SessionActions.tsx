import { useState } from 'react';
import type { Session } from './types';
import { abilitiesFor } from './availability';
import { Button } from '../ui/Button';
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
 * surface change shape as the session moves, and leave someone hunting for a
 * button that was there a moment ago. A disabled key carrying its reason
 * answers the question instead — as part of the button, because a tooltip is
 * unreachable by touch and invisible to anyone scanning (§7), which is what
 * makes each of these a stacked key holding §7's 52px minimum.
 *
 * They are ordered by how much they destroy: reviving and reconnecting change
 * nothing, ending releases the sandbox but keeps the transcript, deleting is
 * final. Only the last asks twice, and it asks inline, in place of the row's
 * own controls — §7 forbids a second dialog over a dialog.
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

  if (confirmingDelete) {
    return (
      <div className="actions">
        <p className="actions__question">Delete this session and its history?</p>
        <Button
          size="sm"
          onClick={() => {
            setConfirmingDelete(false);
          }}
        >
          Keep it
        </Button>
        <Button
          size="sm"
          tone="danger"
          disabled={busy}
          onClick={() => {
            setConfirmingDelete(false);
            onDelete();
          }}
        >
          Delete permanently
        </Button>
      </div>
    );
  }

  return (
    <div className="actions">
      {/*
       * Revive is the primary action wherever it is offered (§11), which is
       * where this surface spends the one accent button §7 allows it.
       */}
      {can.canRevive ? (
        <Button
          tone="accent"
          disabled={busy}
          onClick={onRevive}
          reason="Start it again and carry on where you left off"
        >
          Revive session
        </Button>
      ) : (
        <Button
          disabled={busy || !can.canReconnect}
          onClick={onReconnect}
          reason={
            can.canReconnect
              ? 'Reattach to this session’s sandbox'
              : 'Available once the session is running'
          }
        >
          Reconnect
        </Button>
      )}

      <Button
        disabled={busy || !can.canStop}
        onClick={onStop}
        reason={
          can.canStop ? 'Releases the sandbox, keeps the history' : 'This session is not running'
        }
      >
        End session
      </Button>

      <Button
        tone="danger"
        disabled={busy || !can.canDelete}
        onClick={() => {
          // Asked twice because this is the only act that cannot be undone.
          setConfirmingDelete(true);
        }}
        reason={
          can.canDelete
            ? 'Removes the session and its history'
            : 'Not while the session is being moved'
        }
      >
        Delete session
      </Button>
    </div>
  );
}
