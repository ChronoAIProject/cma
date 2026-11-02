import { useState } from 'react';
import type { User } from '../auth/types';
import { canOpenOperations } from '../operator/capabilities';
import type { AppRoute } from '../routing/routes';
import { Menu, type MenuEntry } from '../ui/Menu';
import {
  IconDarkOutline16,
  IconBrowseOutline16,
  IconFollowsystemOutline16,
  IconLightOutline16,
  IconCopyOutline16,
  IconRightUpOutline16,
  IconSettingsOutline16,
} from '../ui/icons';
import {
  APPEARANCE_ORDER,
  appearanceLabel,
  setAppearance,
  storedAppearance,
  type Appearance,
} from '../theme/appearance';
import { BUILD } from '../shell/build';
import './ProfileMenu.css';
import {
  SYSTEM_TEXT_SIZES,
  setTextSize,
  textSizeLabel,
  useSystemTextSize,
} from '../theme/textSize';

interface ProfileMenuProps {
  readonly user: User;
  readonly onSignOut: () => void;
  readonly onNavigate?: (route: AppRoute) => void;
  readonly collapsed?: boolean;
}

const MENU_ROUTES: Readonly<Record<string, AppRoute>> = {
  docs: { kind: 'docs' },
  operations: { kind: 'operations' },
  settings: { kind: 'settings', tab: 'general' },
};

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

export function ProfileMenu({ user, onSignOut, onNavigate, collapsed = false }: ProfileMenuProps) {
  const [open, setOpen] = useState(false);
  const [appearance, setLocalAppearance] = useState<Appearance>(() => storedAppearance());
  const [avatarBroken, setAvatarBroken] = useState(false);
  const [copied, setCopied] = useState(false);
  const textSize = useSystemTextSize();
  const canOperate = canOpenOperations(user.capabilities);
  const items: MenuEntry[] = [
    { id: 'docs', icon: <IconBrowseOutline16 />, label: 'Docs' },
    { id: 'settings', icon: <IconSettingsOutline16 />, label: 'Settings' },
    ...(canOperate
      ? [{ id: 'operations', icon: <IconSettingsOutline16 />, label: 'Operations' }]
      : []),
    { type: 'separator', id: 'before-preferences' },
    {
      id: 'appearance',
      icon: appearanceIcon(appearance),
      label: `Appearance: ${appearanceLabel(appearance)}`,
    },
    {
      type: 'choices',
      id: 'system-text',
      text: 'System text size',
      selectedId: `text-${textSize}`,
      choices: SYSTEM_TEXT_SIZES.map((size) => ({
        id: `text-${size}`,
        label: textSizeLabel(size),
      })),
    },
    { type: 'separator', id: 'before-sign-out' },
    {
      id: 'build',
      icon: <IconCopyOutline16 />,
      label: copied ? 'Build revision copied' : `Build ${BUILD}`,
    },
    { id: 'sign-out', icon: <IconRightUpOutline16 />, label: 'Sign out' },
  ];

  const choose = (id: string) => {
    const size = SYSTEM_TEXT_SIZES.find((value) => id === `text-${value}`);
    if (size) {
      setTextSize(size);
      return;
    }
    const route = MENU_ROUTES[id];
    if (route !== undefined) {
      setOpen(false);
      onNavigate?.(route);
      return;
    }
    if (id === 'appearance') {
      const at = APPEARANCE_ORDER.indexOf(appearance);
      const next = APPEARANCE_ORDER[(at + 1) % APPEARANCE_ORDER.length] ?? 'system';
      setLocalAppearance(next);
      setAppearance(next);
      return;
    }
    if (id === 'build') {
      // The revision is what a bug report needs, so it copies in one step.
      // A refused clipboard leaves the visible text, which can be read out.
      void navigator.clipboard?.writeText(BUILD).then(
        () => setCopied(true),
        () => setCopied(false),
      );
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
        setCopied(false);
      }}
      anchor={
        <button
          type="button"
          className="profile__trigger"
          aria-haspopup="menu"
          aria-expanded={open}
          aria-label={`Account menu for ${user.displayName ?? user.handle}`}
          onClick={() => {
            if (!open) setLocalAppearance(storedAppearance());
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
