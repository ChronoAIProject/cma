import { useEffect, useId, useRef, useState } from 'react';
import type { Session } from './types';
import './SessionActions.css';

/**
 * What can be done to the session currently open.
 *
 * Three acts that are easy to confuse and must not be: ending one stops its
 * sandbox but keeps the conversation readable, removing one destroys it, and
 * starting again opens a fresh session. Only removal is irreversible, so only
 * removal asks twice.
 *
 * Kept in a menu rather than as three buttons in the tab strip. They are
 * infrequent and consequential, and a destructive control sitting permanently
 * next to a tab is a control someone eventually hits by accident.
 */

interface SessionActionsProps {
  readonly session: Session;
  readonly onTerminate: () => void;
  readonly onDelete: () => void;
  readonly onStartAgain: () => void;
  readonly busy?: boolean;
}

export function SessionActions({
  session,
  onTerminate,
  onDelete,
  onStartAgain,
  busy = false,
}: SessionActionsProps) {
  const [open, setOpen] = useState(false);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const menuId = useId();

  /** Close, and forget any primed confirmation. */
  const close = () => {
    setOpen(false);
    // Reopening must never land on an armed destructive action.
    setConfirmingDelete(false);
  };

  useEffect(() => {
    if (!open) {
      return;
    }
    const onPointerDown = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) {
        close();
      }
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        close();
      }
    };
    document.addEventListener('pointerdown', onPointerDown);
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('pointerdown', onPointerDown);
      document.removeEventListener('keydown', onKeyDown);
    };
  }, [open]);

  const ended = session.lifecycle === 'terminated';

  return (
    <div className="actions" ref={rootRef}>
      <button
        type="button"
        className="actions__trigger"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? menuId : undefined}
        disabled={busy}
        onClick={() => {
          if (open) {
            close();
          } else {
            setOpen(true);
          }
        }}
      >
        <span className="sr-only">Actions for this session</span>
        <span aria-hidden="true">⋯</span>
      </button>

      {open ? (
        <div className="actions__menu" id={menuId} role="menu">
          {ended ? (
            <button
              type="button"
              className="actions__item"
              role="menuitem"
              onClick={() => {
                close();
                onStartAgain();
              }}
            >
              Start a new session like this
            </button>
          ) : (
            <button
              type="button"
              className="actions__item"
              role="menuitem"
              onClick={() => {
                close();
                onTerminate();
              }}
            >
              End session
              <span className="actions__hint">Stops the sandbox, keeps the transcript</span>
            </button>
          )}

          {confirmingDelete ? (
            <button
              type="button"
              className="actions__item actions__item--danger"
              role="menuitem"
              onClick={() => {
                close();
                onDelete();
              }}
            >
              Delete permanently — confirm
              <span className="actions__hint">This cannot be undone</span>
            </button>
          ) : (
            <button
              type="button"
              className="actions__item actions__item--danger"
              role="menuitem"
              onClick={() => {
                // Asked twice because this is the one act that cannot be
                // undone. Ending a session is recoverable; this is not.
                setConfirmingDelete(true);
              }}
            >
              Delete session
              <span className="actions__hint">Removes the session and its history</span>
            </button>
          )}
        </div>
      ) : null}
    </div>
  );
}
