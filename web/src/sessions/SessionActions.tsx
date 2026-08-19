import { useEffect, useId, useRef, useState } from 'react';
import type { KernelOffer, Session } from './types';
import { abilitiesFor } from './availability';
import { switchFor, unavailableBecause } from './planeSwitch';
import { Button } from '../ui/Button';
import './SessionActions.css';

/**
 * What can be done to the session being described — docs/Web_Style.md §12.
 *
 * One primary, one secondary, and the overflow menu (§7): a header carrying
 * four equally weighted keys makes the reader choose before they can read,
 * which is what this header used to do. The primary is Revive where §11
 * offers it — revive is the primary action wherever it is offered — and
 * Reconnect otherwise. The secondary is Details, which opens the drawer.
 * Everything else — moving the session's plane, ending it — lives behind the
 * `···` key. Delete lives at the drawer's foot, away from everything
 * reversible.
 *
 * **Every disabled control states why, and one sentence covers all of them.**
 * `abilitiesFor` is the single source of enablement and it disables controls
 * for exactly one cause at a time — the session's state — so the reason is
 * drawn once per row, as a `.control-reason`, and each control points at it.
 *
 * The plane move confirms inline, in place of the row's own controls
 * (§18.6), stating the two costs that are not obvious. §7 forbids a second
 * dialog over a dialog, and the same rule holds for a header.
 */

interface SessionActionsProps {
  readonly session: Session;
  readonly onReconnect: () => void;
  readonly onRevive: () => void;
  readonly onStop: () => void;
  /** Opens and closes the details drawer. */
  readonly onDetails: () => void;
  readonly detailsOpen?: boolean;
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
  onDetails,
  detailsOpen = false,
  kernels = [],
  onChangePlane,
  busy = false,
}: SessionActionsProps) {
  const [confirmingSwitch, setConfirmingSwitch] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  const reasonId = useId();
  const can = abilitiesFor(session);
  // Absent rather than disabled where the kernel does not advertise the other
  // plane (§18.6): a control that can never work is not one that is currently
  // unavailable.
  const move = onChangePlane === undefined ? undefined : switchFor(session, kernels);

  // The menu closes on Escape and on a press anywhere outside it, because a
  // plate that only its own key can dismiss traps the pointer.
  useEffect(() => {
    if (!menuOpen) {
      return;
    }
    const onPress = (event: MouseEvent) => {
      if (menuRef.current !== null && !menuRef.current.contains(event.target as Node)) {
        setMenuOpen(false);
      }
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        setMenuOpen(false);
      }
    };
    document.addEventListener('mousedown', onPress);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onPress);
      document.removeEventListener('keydown', onKey);
    };
  }, [menuOpen]);

  if (confirmingSwitch && move !== undefined) {
    /*
     * §18.6: it confirms inline, in place of the header's own controls, and
     * states the two things that are true and not obvious. Neither is
     * recoverable by pressing the key again, which is why they are said
     * before rather than reported after.
     */
    return (
      <div className="actions">
        <span className="control-reason actions__question">
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

  /*
   * §7: one reason per row, beside the controls it explains. The session's
   * state disables for one cause at a time, so one sentence covers the
   * primary and the menu's items alike.
   */
  const describedBy = can.reason === undefined ? undefined : reasonId;

  return (
    <div className="actions">
      {can.reason === undefined ? null : (
        <span className="control-reason actions__reason" id={reasonId}>
          {can.reason}
        </span>
      )}

      {/*
       * Revive is the primary action wherever it is offered (§11), which is
       * where this surface spends the one primary key §7 allows it.
       */}
      {can.canRevive ? (
        <Button variant="primary" disabled={busy} onClick={onRevive} aria-describedby={describedBy}>
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

      <Button onClick={onDetails} aria-expanded={detailsOpen}>
        Details
      </Button>

      <div className="actions__overflow" ref={menuRef}>
        <Button
          placement="button--icon"
          aria-haspopup="menu"
          aria-expanded={menuOpen}
          aria-label="More actions"
          onClick={() => {
            setMenuOpen((open) => !open);
          }}
        >
          ···
        </Button>
        {menuOpen ? (
          <div className="menu actions__menu" role="menu" aria-label="More actions">
            {/*
             * §18.6: a small key naming the plane it moves to, absent — not
             * disabled — when the kernel does not advertise the other plane,
             * and disabled with its reason when the state refuses it.
             */}
            {move === undefined ? null : (
              <button
                type="button"
                role="menuitem"
                className="menu__item"
                aria-disabled={busy || !move.available}
                onClick={() => {
                  if (busy || !move.available) {
                    return;
                  }
                  setMenuOpen(false);
                  setConfirmingSwitch(true);
                }}
              >
                {move.label}
                {move.available ? null : (
                  <span className="menu__kind">{unavailableBecause(session)}</span>
                )}
              </button>
            )}
            <button
              type="button"
              role="menuitem"
              className="menu__item"
              aria-disabled={busy || !can.canStop}
              aria-describedby={describedBy}
              onClick={() => {
                if (busy || !can.canStop) {
                  return;
                }
                setMenuOpen(false);
                onStop();
              }}
            >
              End session
            </button>
          </div>
        ) : null}
      </div>
    </div>
  );
}
