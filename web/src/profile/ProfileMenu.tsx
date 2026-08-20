import { useState } from 'react';
import type { User } from '../auth/types';
import { Menu, type MenuEntry } from '../ui/Menu';
import {
  IconAgentPresetOutline16,
  IconCodeOutline16,
  IconDarkOutline16,
  IconDataOutline16,
  IconFollowsystemOutline16,
  IconLightOutline16,
  IconRightUpOutline16,
} from '../ui/icons';
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
 * The account row at the foot of the sidebar, and its menu.
 *
 * It is a menu of account actions, not navigation. `Web_Style.md` is explicit
 * that it must not grow into a nav menu, because the product is one page.
 *
 * **Built on `Menu` rather than beside it.** This used to render its own
 * `<div className="menu" role="menu">` with its own outside-click handling and
 * its own focus move, and consequently had none of what the primitive already
 * offers: placement that stays inside the window, a leading glyph per row,
 * arrow-key movement, headings and separators. It opened downward from the
 * foot of a column and ran off the bottom of the screen, which is what a
 * hand-rolled copy of a solved problem looks like from the outside.
 *
 * The trigger says who is signed in. On a service that spends the owner's own
 * vendor subscription, which account is in use is the fact most worth being
 * able to check without opening anything.
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
  /**
   * Open the operator panel, where the caller may operate at all.
   *
   * Omitted rather than disabled for anybody else: §12 draws no dead key for
   * something that will never work, and the capability decides whether this is
   * offered rather than whether it is greyed.
   */
  readonly onOperations?: () => void;
  /** Whether the operator panel is the surface currently showing. */
  readonly operationsActive?: boolean;
  /**
   * Whether the sidebar it sits in is closed.
   *
   * Taken rather than read off the sidebar's own class: a sheet that styles by
   * another component's modifier is coupled to a name it does not own.
   */
  readonly collapsed?: boolean;
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

/** The glyph for the appearance a click would move to. */
function appearanceIcon(appearance: Appearance) {
  if (appearance === 'light') {
    return <IconLightOutline16 />;
  }
  if (appearance === 'dark') {
    return <IconDarkOutline16 />;
  }
  return <IconFollowsystemOutline16 />;
}

export function ProfileMenu({
  user,
  onSignOut,
  onTerminalThemeChange,
  onManageEnvironments,
  onOperations,
  operationsActive = false,
  collapsed = false,
}: ProfileMenuProps) {
  const [open, setOpen] = useState(false);
  const [appearance, setLocalAppearance] = useState<Appearance>(() => storedAppearance());
  // An avatar that fails to load must fall back to initials rather than leave
  // a broken image where a person's face should be.
  const [avatarBroken, setAvatarBroken] = useState(false);
  const [terminalThemeId, setTerminalThemeId] = useState<string | null>(() => storedThemeId());

  const surface = effectiveAppearance(appearance);
  const theme = resolveTheme(surface, terminalThemeId);

  const items: MenuEntry[] = [
    {
      id: 'appearance',
      icon: appearanceIcon(appearance),
      label: `Appearance: ${appearanceLabel(appearance)}`,
    },
    {
      id: 'terminal-theme',
      icon: <IconCodeOutline16 />,
      label: `Terminal theme: ${theme.label}`,
    },
  ];
  if (onManageEnvironments !== undefined) {
    items.push({
      id: 'environments',
      icon: <IconAgentPresetOutline16 />,
      label: 'Environment profiles',
    });
  }
  /*
   * Operations sits here now. It was the app bar's view switch, which §9
   * retired along with the bar — leaving an account menu that omitted the one
   * place an operator goes, beside a switch with nowhere left to live.
   *
   * It is still a view rather than an account action, so it is separated from
   * the two settings above it and marked when it is the surface showing.
   */
  if (onOperations !== undefined) {
    items.push({ type: 'separator', id: 'after-settings' });
    items.push({
      id: 'operations',
      icon: <IconDataOutline16 />,
      /*
       * The row names where it goes rather than marking itself as current. A
       * check mark would make this a value among alternatives — the primitive
       * gives every row `menuitemradio` the moment a selection is passed — and
       * these are actions. Saying "back to sessions" is also the plainer
       * answer to "what happens if I press this".
       */
      label: operationsActive ? 'Back to sessions' : 'Operations',
    });
  }
  items.push({ type: 'separator', id: 'before-sign-out' });
  items.push({ id: 'sign-out', icon: <IconRightUpOutline16 />, label: 'Sign out' });

  const choose = (id: string) => {
    switch (id) {
      case 'appearance': {
        // Cycles rather than opening a submenu: three states is few enough
        // that a second layer of chrome would cost more than it explains, and
        // the current one is always named on the row. The menu stays open,
        // because cycling is something somebody does more than once.
        const next =
          APPEARANCE_ORDER[(APPEARANCE_ORDER.indexOf(appearance) + 1) % APPEARANCE_ORDER.length] ??
          'system';
        setLocalAppearance(next);
        setAppearance(next);
        return;
      }
      case 'terminal-theme': {
        // Only palettes suiting the current appearance are offered, so a dark
        // theme can never be placed on a light page.
        const options = themesFor(surface);
        const next = options[(options.findIndex((t) => t.id === theme.id) + 1) % options.length];
        if (next !== undefined) {
          setTerminalThemeId(next.id);
          storeThemeId(next.id);
          // The palette is read when a terminal is constructed, so the change
          // lands on the next one rather than repainting a live canvas
          // underneath someone mid-command.
          onTerminalThemeChange?.(next.id);
        }
        return;
      }
      case 'environments':
        setOpen(false);
        onManageEnvironments?.();
        return;
      case 'operations':
        setOpen(false);
        onOperations?.();
        return;
      case 'sign-out':
        setOpen(false);
        onSignOut();
        return;
      default:
    }
  };

  return (
    <Menu
      open={open}
      /*
       * Upward, and aligned to the trigger's leading edge. This sits at the
       * foot of a column, so there is nowhere below it to go — it used to open
       * downward and put `Sign out` past the bottom of the window.
       */
      side="top"
      align="start"
      label="Account"
      placement={`profile${collapsed ? ' profile--collapsed' : ''}`}
      matchAnchorWidth
      items={items}
      onSelect={choose}
      onClose={() => {
        setOpen(false);
      }}
      anchor={
        <button
          type="button"
          className="profile__trigger"
          aria-haspopup="menu"
          aria-expanded={open}
          aria-label={`Account menu for ${user.displayName ?? user.handle}`}
          onClick={() => {
            setOpen((was) => !was);
          }}
        >
          <span className="profile__face" aria-hidden="true">
            {user.avatarUrl === undefined || avatarBroken ? (
              initialsFor(user)
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
          </span>
          {/*
           * Both lines truncate rather than wrap. The sidebar is resizable and
           * an address is arbitrarily long, so a wrapping row would change the
           * height of the column's foot as somebody drags it.
           */}
          <span className="profile__who">
            <span className="profile__name">{user.displayName ?? user.handle}</span>
            {user.email === undefined || user.email === '' ? null : (
              <span className="profile__email">{user.email}</span>
            )}
          </span>
        </button>
      }
    />
  );
}
