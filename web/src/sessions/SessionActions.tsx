import { useId, useState } from 'react';
import type { KernelOffer, Session } from './types';
import { abilitiesFor } from './availability';
import { switchFor, unavailableBecause } from './planeSwitch';
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
  /**
   * What this deployment runs, and what each runtime can be.
   *
   * Decides whether a plane switch is offered at all. Empty means nothing is
   * known, and nothing known draws no switch: offering a move that will be
   * refused is worse than not offering one.
   */
  readonly kernels?: readonly KernelOffer[];
  /** Move this session onto the other plane. */
  readonly onChangePlane?: (to: 'terminal' | 'conversation') => void;
  readonly busy?: boolean;
}

export function SessionActions({
  session,
  onReconnect,
  onRevive,
  onStop,
  onDelete,
  kernels = [],
  onChangePlane,
  busy = false,
}: SessionActionsProps) {
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const [confirmingSwitch, setConfirmingSwitch] = useState(false);
  const reasonId = useId();
  const can = abilitiesFor(session);
  // Absent rather than disabled where the kernel does not advertise the other
  // plane (§18.6): a control that can never work is not one that is currently
  // unavailable.
  const move = onChangePlane === undefined ? undefined : switchFor(session, kernels);
  // Points every control at the one sentence explaining the state that
  // disabled it, so the connection is announced rather than merely adjacent.
  const describedBy = can.reason === undefined ? undefined : reasonId;

  if (confirmingSwitch && move !== undefined) {
    /*
     * §18.6: it confirms inline, in place of the header's own controls, and
     * states the two things that are true and not obvious. Neither is
     * recoverable by pressing the key again, which is why they are said before
     * rather than reported after.
     */
    return (
      <div className="actions">
        <span className="actions__question actions__question--cost">
          Move to {move.label.toLowerCase()}? The turn in flight is lost, and this session is not
          interactive while it moves.
        </span>
        <Button
          size="sm"
          onClick={() => {
            setConfirmingSwitch(false);
          }}
        >
          Stay here
        </Button>
        <Button
          size="sm"
          disabled={busy}
          onClick={() => {
            setConfirmingSwitch(false);
            onChangePlane?.(move.to);
          }}
        >
          Move to {move.label.toLowerCase()}
        </Button>
      </div>
    );
  }

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
       * §18.6: a quiet small key naming the plane it moves to, beside the
       * lifecycle controls. Quiet because the accent this surface is allowed
       * belongs to revive, and because moving planes is a considered act
       * rather than the thing this header exists for.
       */}
      {move === undefined ? null : (
        <Button
          size="sm"
          disabled={busy || !move.available}
          reason={move.available ? undefined : unavailableBecause(session)}
          onClick={() => {
            setConfirmingSwitch(true);
          }}
        >
          {move.label}
        </Button>
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
