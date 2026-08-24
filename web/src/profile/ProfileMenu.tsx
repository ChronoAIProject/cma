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
  setAppearance,
  storedAppearance,
  type Appearance,
} from '../theme/appearance';
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
  /** Open the repository-authored SDK documentation surface. */
  readonly onDeveloper?: () => void;
  /** Whether the Developer surface is currently showing. */
  readonly developerActive?: boolean;
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
  onManageEnvironments,
  onOperations,
  operationsActive = false,
  onDeveloper,
  developerActive = false,
  collapsed = false,
}: ProfileMenuProps) {
  const [open, setOpen] = useState(false);
  const [appearance, setLocalAppearance] = useState<Appearance>(() => storedAppearance());
  // An avatar that fails to load must fall back to initials rather than leave
  // a broken image where a person's face should be.
  const [avatarBroken, setAvatarBroken] = useState(false);

  const items: MenuEntry[] = [
    {
      id: 'appearance',
      icon: appearanceIcon(appearance),
      label: `Appearance: ${appearanceLabel(appearance)}`,
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
   * Developer and Operations are surfaces rather than account settings, so a
   * separator keeps them out of the settings group. They still remain actions:
   * each row names where it goes instead of pretending this menu is navigation.
   */
  if (onDeveloper !== undefined || onOperations !== undefined) {
    items.push({ type: 'separator', id: 'after-settings' });
  }
  if (onDeveloper !== undefined) {
    items.push({
      id: 'developer',
      icon: <IconCodeOutline16 />,
      label: developerActive ? 'Back to sessions' : 'Developer',
    });
  }
  if (onOperations !== undefined) {
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
      case 'environments':
        setOpen(false);
        onManageEnvironments?.();
        return;
      case 'operations':
        setOpen(false);
        onOperations?.();
        return;
      case 'developer':
        setOpen(false);
        onDeveloper?.();
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
