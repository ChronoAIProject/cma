import { useEffect, useId, useRef, useState } from 'react';
import type { User } from '../auth/types';
import {
  APPEARANCE_ORDER,
  appearanceLabel,
  effectiveAppearance,
  setAppearance,
  storedAppearance,
  type Appearance,
} from '../theme/appearance';
import { resolveTheme, storeThemeId, storedThemeId, themesFor } from '../terminal/themes';
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
  /**
   * Told when the terminal palette changes, so the surface can be rebuilt.
   *
   * Optional because the menu is useful without a terminal behind it — the
   * preference is still stored either way.
   */
  readonly onTerminalThemeChange?: (id: string) => void;
  /**
   * Open the environment profile manager.
   *
   * An account action rather than navigation: it opens a dialog over the same
   * page and returns focus here when that dialog closes. The product is one
   * page and this must not grow a navigation menu.
   */
  readonly onManageEnvironments?: () => void;
}

/**
 * Two characters standing in for a face.
 *
 * A person's name initials first, since that is what they are called. Failing
 * that, the start of their email — which is still recognisably theirs, unlike
 * the subject identifier, which would render as two arbitrary hex digits and
 * look like a bug.
 */
export function initialsFor(user: Pick<User, 'displayName' | 'email' | 'handle'>): string {
  const name = user.displayName?.trim();
  if (name !== undefined && name !== '') {
    const parts = name.split(/[\s._-]+/).filter((p) => p.length > 0);
    if (parts.length >= 2) {
      return `${parts[0]?.[0] ?? ''}${parts[1]?.[0] ?? ''}`.toUpperCase();
    }
    return name.slice(0, 2).toUpperCase();
  }

  const email = user.email?.trim();
  if (email !== undefined && email !== '') {
    const local = email.split('@')[0] ?? email;
    return local.slice(0, 2).toUpperCase();
  }

  const fallback = user.handle.trim();
  if (fallback === '') {
    return '?';
  }
  const parts = fallback.split(/[\s._-]+/).filter((p) => p.length > 0);
  if (parts.length >= 2) {
    return `${parts[0]?.[0] ?? ''}${parts[1]?.[0] ?? ''}`.toUpperCase();
  }
  return fallback.slice(0, 2).toUpperCase();
}

export function ProfileMenu({
  user,
  onSignOut,
  onTerminalThemeChange,
  onManageEnvironments,
}: ProfileMenuProps) {
  const [open, setOpen] = useState(false);
  const [appearance, setLocalAppearance] = useState<Appearance>(() => storedAppearance());
  // An avatar that fails to load must fall back to initials rather than leave
  // a broken image where a person's face should be.
  const [avatarBroken, setAvatarBroken] = useState(false);
  const [terminalThemeId, setTerminalThemeId] = useState<string | null>(() => storedThemeId());
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
        {user.avatarUrl === undefined || avatarBroken ? (
          <span aria-hidden="true">{initialsFor(user)}</span>
        ) : (
          <img
            className="profile__avatar"
            src={user.avatarUrl}
            alt=""
            onError={() => {
              setAvatarBroken(true);
            }}
          />
        )}
      </button>

      {open ? (
        <div className="menu profile__menu" id={menuId} role="menu">
          <p className="profile__handle" role="presentation">
            {user.handle}
          </p>
          {user.email !== undefined && user.email !== user.handle ? (
            <p className="profile__email" role="presentation">
              {user.email}
            </p>
          ) : null}
          <button
            type="button"
            className="menu__item profile__item"
            role="menuitem"
            onClick={() => {
              // Cycles rather than opening a submenu: three states is few
              // enough that a second layer of chrome would cost more than it
              // explains, and the current one is always named on the control.
              const next =
                APPEARANCE_ORDER[
                  (APPEARANCE_ORDER.indexOf(appearance) + 1) % APPEARANCE_ORDER.length
                ] ?? 'system';
              setLocalAppearance(next);
              setAppearance(next);
            }}
          >
            Appearance: {appearanceLabel(appearance)}
          </button>
          <button
            type="button"
            className="menu__item profile__item"
            role="menuitem"
            onClick={() => {
              // Only palettes suiting the current appearance are offered, so a
              // dark theme can never be placed on a light page.
              const surface = effectiveAppearance(appearance);
              const options = themesFor(surface);
              const current = resolveTheme(surface, terminalThemeId);
              const next =
                options[(options.findIndex((t) => t.id === current.id) + 1) % options.length];
              if (next !== undefined) {
                setTerminalThemeId(next.id);
                storeThemeId(next.id);
                // The palette is read when a terminal is constructed, so the
                // change lands on the next one rather than repainting a live
                // canvas underneath someone mid-command.
                onTerminalThemeChange?.(next.id);
              }
            }}
          >
            Terminal theme: {resolveTheme(effectiveAppearance(appearance), terminalThemeId).label}
          </button>
          {onManageEnvironments !== undefined ? (
            <button
              type="button"
              className="menu__item profile__item"
              role="menuitem"
              onClick={() => {
                setOpen(false);
                onManageEnvironments();
              }}
            >
              Environment profiles
            </button>
          ) : null}
          {/*
           * Operations is not here. §14 makes it a page and one of §1's two
           * views, so it is reached by the app bar's switch rather than by an
           * account action — a view somebody moves to, not something they do
           * to their account.
           */}
          <button
            type="button"
            ref={firstItemRef}
            className="menu__item profile__item"
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
