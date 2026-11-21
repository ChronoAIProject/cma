import { useEffect, useRef, useState } from 'react';
import type { User } from '../auth/types';
import { canOpenOperations } from '../operator/capabilities';
import type { AppRoute } from '../routing/routes';
import { Menu, type MenuEntry } from '../ui/Menu';
import {
  IconBrowseOutline16,
  IconCopyOutline16,
  IconRightUpOutline16,
  IconSettingsOutline16,
} from '../ui/icons';
import {
  APPEARANCE_ORDER,
  appearanceLabel,
  setAppearance,
  useAppearance,
} from '../theme/appearance';
import { BUILD_INFO } from '../shell/build';
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
  usage: { kind: 'usage' },
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

export function ProfileMenu({ user, onSignOut, onNavigate, collapsed = false }: ProfileMenuProps) {
  const [open, setOpen] = useState(false);
  const appearance = useAppearance();
  const triggerRef = useRef<HTMLButtonElement>(null);
  const [avatarBroken, setAvatarBroken] = useState(false);
  const [copyState, setCopyState] = useState<'idle' | 'copied' | 'failed'>('idle');
  const fallbackRef = useRef<HTMLElement>(null);
  const copyAttempt = useRef(0);
  const clearCopy = () => {
    copyAttempt.current += 1;
    setCopyState('idle');
  };
  useEffect(() => {
    if (copyState !== 'failed' || !fallbackRef.current) return;
    const range = document.createRange();
    range.selectNodeContents(fallbackRef.current);
    const selection = window.getSelection();
    selection?.removeAllRanges();
    selection?.addRange(range);
  }, [copyState]);
  const textSize = useSystemTextSize();
  const canOperate = canOpenOperations(user.capabilities);
  const items: MenuEntry[] = [
    { id: 'docs', icon: <IconBrowseOutline16 />, label: 'Docs' },
    { id: 'usage', icon: <IconSettingsOutline16 />, label: 'Usage' },
    ...(canOperate
      ? [{ id: 'operations', icon: <IconSettingsOutline16 />, label: 'Operations' }]
      : []),
    { type: 'separator', id: 'before-preferences' },
    {
      type: 'choices',
      id: 'appearance',
      text: 'Appearance',
      selectedId: `appearance-${appearance}`,
      choices: APPEARANCE_ORDER.map((value) => ({
        id: `appearance-${value}`,
        label: appearanceLabel(value),
      })),
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
      label: (
        <span className="profile__build">
          {copyState === 'copied'
            ? 'Build version copied'
            : `Copy build version · ${BUILD_INFO.label}`}
          {copyState === 'failed' && (
            <span className="profile__build-fallback">
              Could not copy. Select the build version to copy it manually.
              <code ref={fallbackRef}>{BUILD_INFO.copyText}</code>
            </span>
          )}
        </span>
      ),
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
      triggerRef.current?.focus();
      onNavigate?.(route);
      return;
    }
    const chosenAppearance = APPEARANCE_ORDER.find((value) => id === `appearance-${value}`);
    if (chosenAppearance) {
      setAppearance(chosenAppearance);
      return;
    }
    if (id === 'build') {
      const attempt = ++copyAttempt.current;
      const copy = async () => {
        try {
          if (!navigator.clipboard?.writeText) throw new Error('Clipboard unavailable');
          await navigator.clipboard.writeText(BUILD_INFO.copyText);
          if (attempt === copyAttempt.current) setCopyState('copied');
        } catch {
          if (attempt === copyAttempt.current) setCopyState('failed');
        }
      };
      void copy();
      return;
    }
    if (id === 'sign-out') {
      setOpen(false);
      onSignOut();
    }
  };

  return (
    <>
      <span className="sr-only" role="status" aria-live="polite">
        {copyState === 'copied'
          ? 'Build version copied'
          : copyState === 'failed'
            ? 'Could not copy. Select the build version to copy it manually.'
            : ''}
      </span>
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
          clearCopy();
        }}
        anchor={
          <button
            type="button"
            className="profile__trigger"
            ref={triggerRef}
            aria-haspopup="menu"
            aria-expanded={open}
            aria-label={`Account menu for ${user.displayName ?? user.handle}`}
            onClick={() => {
              clearCopy();
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
              {user.email === undefined ? null : (
                <span className="profile__email">{user.email}</span>
              )}
            </span>
          </button>
        }
      />
    </>
  );
}
