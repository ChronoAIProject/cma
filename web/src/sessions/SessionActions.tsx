import { useId, useState } from 'react';
import type { Session } from './types';
import { abilitiesFor } from './availability';
import { Button } from '../ui/Button';
import './SessionActions.css';

/**
 * What can be done to the session being described — docs/Web_Style.md §12.
 *
 * In the session header, at the head of the terminal they act on, visible
 * without opening anything. Plain buttons rather than a menu: these are the
 * only actions a session has, there are at most three of them at a time, and
 * hiding them behind a control that has to be discovered means someone reading
 * "revive it to carry on" has nowhere obvious to go.
 *
 * Every action is always shown, and disabled when the session's state does not
 * allow it (§7). Removing a control that does not currently apply would make
 * the header change shape as the session moves, and leave someone hunting for
 * a button that was there a moment ago.
 *
 * **Every disabled control states why, and one sentence covers all of them.**
 * `abilitiesFor` is the single source of enablement and it disables controls
 * for exactly one cause at a time — the session's state — so the reason is
 * drawn once, in the band, and each control points at it. It is on the surface
 * rather than in a tooltip, which §7 rejects as unreachable by touch and
 * invisible to anyone scanning, and it is the same sentence §11 requires a
 * session that cannot be typed into to carry.
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
  const reasonId = useId();
  const can = abilitiesFor(session);
  // Points every control at the one sentence explaining the state that
  // disabled it, so the connection is announced rather than merely adjacent.
  const describedBy = can.reason === undefined ? undefined : reasonId;

  if (confirmingDelete) {
    return (
      <div className="actions">
        <span className="actions__question">Delete this session and its history?</span>
        <Button
          onClick={() => {
            setConfirmingDelete(false);
          }}
        >
          Keep it
        </Button>
        <Button
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
      {can.reason === undefined ? null : (
        <span className="band__value actions__reason" id={reasonId}>
          {can.reason}
        </span>
      )}

      {/*
       * Revive is the primary action wherever it is offered (§11), which is
       * where this surface spends the one accent button §7 allows it.
       */}
      {can.canRevive ? (
        <Button tone="accent" disabled={busy} onClick={onRevive} aria-describedby={describedBy}>
          Revive
        </Button>
      ) : (
        <Button
          disabled={busy || !can.canReconnect}
          onClick={onReconnect}
          aria-describedby={describedBy}
        >
          Reconnect
        </Button>
      )}

      <Button disabled={busy || !can.canStop} onClick={onStop} aria-describedby={describedBy}>
        End
      </Button>

      <Button
        tone="danger"
        disabled={busy || !can.canDelete}
        aria-describedby={describedBy}
        onClick={() => {
          // Asked twice because this is the only act that cannot be undone.
          setConfirmingDelete(true);
        }}
      >
        Delete
      </Button>
    </div>
  );
}
