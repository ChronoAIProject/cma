import { useState } from 'react';
import type { User } from '../auth/types';
import { Menu, type MenuEntry } from '../ui/Menu';
import {
  IconDarkOutline16,
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

interface ProfileMenuProps {
  readonly user: User;
  readonly onSignOut: () => void;
  readonly collapsed?: boolean;
}

export function initialsFor(user: Pick<User, 'displayName' | 'email' | 'handle'>): string {
  const source =
    user.displayName?.trim() || user.email?.split('@')[0]?.trim() || user.handle.trim();
  if (source === '') return '?';
  const parts = source.split(/[\s._-]+/).filter(Boolean);
  return parts.length > 1
    ? `${parts[0]?.[0] ?? ''}${parts[1]?.[0] ?? ''}`.toUpperCase()
    : source.slice(0, 2).toUpperCase();
}

function appearanceIcon(appearance: Appearance) {
  if (appearance === 'light') return <IconLightOutline16 />;
  if (appearance === 'dark') return <IconDarkOutline16 />;
  return <IconFollowsystemOutline16 />;
}

export function ProfileMenu({ user, onSignOut, collapsed = false }: ProfileMenuProps) {
  const [open, setOpen] = useState(false);
  const [appearance, setLocalAppearance] = useState<Appearance>(() => storedAppearance());
  const [avatarBroken, setAvatarBroken] = useState(false);
  const items: MenuEntry[] = [
    {
      id: 'appearance',
      icon: appearanceIcon(appearance),
      label: `Appearance: ${appearanceLabel(appearance)}`,
    },
    { type: 'separator', id: 'before-sign-out' },
    { id: 'sign-out', icon: <IconRightUpOutline16 />, label: 'Sign out' },
  ];

  const choose = (id: string) => {
    if (id === 'appearance') {
      const at = APPEARANCE_ORDER.indexOf(appearance);
      const next = APPEARANCE_ORDER[(at + 1) % APPEARANCE_ORDER.length] ?? 'system';
      setLocalAppearance(next);
      setAppearance(next);
      return;
    }
    if (id === 'sign-out') {
      setOpen(false);
      onSignOut();
    }
  };

  return (
    <Menu
      open={open}
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
            setOpen((wasOpen) => !wasOpen);
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
          <span className="profile__who">
            <span className="profile__name">{user.displayName ?? user.handle}</span>
            {user.email === undefined ? null : <span className="profile__email">{user.email}</span>}
          </span>
        </button>
      }
    />
  );
}
