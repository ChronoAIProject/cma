import { useEffect, useId, useRef, useState } from 'react';
import type { User } from '../auth/types';
import './ProfileMenu.css';

/**
 * The avatar trigger and its dropdown — the only chrome on the page besides
 * the session tabs.
 *
 * It is a menu of account actions, not navigation. Web_Style.md is explicit
 * that it must not grow into a nav menu, because the product is one page.
 */

interface ProfileMenuProps {
  readonly user: User;
  readonly onSignOut: () => void;
}

function initials(handle: string): string {
  const cleaned = handle.trim();
  if (cleaned.length === 0) {
    return '?';
  }
  const parts = cleaned.split(/[\s._-]+/).filter((p) => p.length > 0);
  if (parts.length >= 2) {
    return `${parts[0]?.[0] ?? ''}${parts[1]?.[0] ?? ''}`.toUpperCase();
  }
  return cleaned.slice(0, 2).toUpperCase();
}

export function ProfileMenu({ user, onSignOut }: ProfileMenuProps) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const firstItemRef = useRef<HTMLButtonElement>(null);
  const menuId = useId();

  // Move focus into the menu on open so a keyboard user is not left behind on
  // the trigger with no indication anything happened.
  useEffect(() => {
    if (open) {
      firstItemRef.current?.focus();
    }
  }, [open]);

  useEffect(() => {
    if (!open) {
      return;
    }

    const onPointerDown = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) {
        setOpen(false);
      }
    };

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        setOpen(false);
        // Escape must return focus to where it came from, or the user is
        // dropped at the top of the document.
        triggerRef.current?.focus();
      }
    };

    const onFocusIn = (event: FocusEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) {
        setOpen(false);
      }
    };

    document.addEventListener('pointerdown', onPointerDown);
    document.addEventListener('keydown', onKeyDown);
    document.addEventListener('focusin', onFocusIn);
    return () => {
      document.removeEventListener('pointerdown', onPointerDown);
      document.removeEventListener('keydown', onKeyDown);
      document.removeEventListener('focusin', onFocusIn);
    };
  }, [open]);

  return (
    <div className="profile" ref={rootRef}>
      <button
        type="button"
        ref={triggerRef}
        className="profile__trigger"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? menuId : undefined}
        onClick={() => {
          setOpen((v) => !v);
        }}
      >
        <span className="sr-only">Account menu for {user.handle}</span>
        {user.avatarUrl === undefined ? (
          <span aria-hidden="true">{initials(user.handle)}</span>
        ) : (
          <img className="profile__avatar" src={user.avatarUrl} alt="" />
        )}
      </button>

      {open ? (
        <div className="profile__menu" id={menuId} role="menu">
          <p className="profile__handle" role="presentation">
            {user.handle}
          </p>
          <button
            type="button"
            ref={firstItemRef}
            className="profile__item"
            role="menuitem"
            onClick={() => {
              setOpen(false);
              onSignOut();
            }}
          >
            Sign out
          </button>
        </div>
      ) : null}
    </div>
  );
}
