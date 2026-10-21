import { useState } from 'react';
import type { User } from '../auth/types';

export type MessageUser = Pick<User, 'displayName' | 'handle' | 'avatarUrl'>;

interface Identity {
  readonly name: string;
  readonly initials: string;
  readonly avatarUrl?: string;
  /** The Agent is drawn as the CMA mark rather than an image or initials. */
  readonly agent?: true;
}

function initials(name: string): string {
  const parts = name
    .trim()
    .split(/[\s._-]+/)
    .filter(Boolean);
  return (
    parts.length > 1
      ? `${Array.from(parts[0]!)[0] ?? ''}${Array.from(parts[1]!)[0] ?? ''}`
      : Array.from(name).slice(0, 2).join('')
  ).toLocaleUpperCase();
}

function safeAvatar(source: string | undefined): string | undefined {
  const value = source?.trim();
  if (!value || value.includes('\\')) return undefined;
  if (value.startsWith('/') && !value.startsWith('//')) return value;
  try {
    const url = new URL(value);
    return url.protocol === 'https:' && !url.username && !url.password ? url.href : undefined;
  } catch {
    return undefined;
  }
}

export function messageIdentity(
  role: 'user' | 'assistant',
  user?: MessageUser,
  unknownUserName = 'You',
): Identity {
  if (role === 'assistant') return { name: 'Chrono Agent', initials: 'CA', agent: true };
  const name = user?.displayName?.trim() || user?.handle.trim() || unknownUserName;
  return { name, initials: initials(name), avatarUrl: safeAvatar(user?.avatarUrl) };
}

/**
 * The CMA mark (`assets/brand/cma-mark.svg`, §21) as the Agent's face.
 *
 * Inline rather than an image so it paints with `currentColor` on a circle
 * the sheet draws from theme aliases. The raster capture it replaces carried
 * its own black ground, which read as a black square in the light theme.
 */
function AgentMark() {
  return (
    <svg viewBox="0 0 100 100" width="20" height="20" focusable="false">
      <path
        d="M71 71 A30 30 0 1 0 29 71"
        fill="none"
        stroke="currentColor"
        strokeWidth="11"
        strokeLinecap="round"
      />
      <path
        d="M71 71 L61 59 L50 71 L39 59 L29 71"
        fill="none"
        stroke="currentColor"
        strokeWidth="10"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <rect x="30" y="38" width="17" height="12" rx="4" fill="currentColor" />
      <rect x="53" y="38" width="17" height="12" rx="4" fill="currentColor" />
      <path d="M46 44 L54 44" fill="none" stroke="currentColor" strokeWidth="4" />
    </svg>
  );
}

export function MessageAvatar({ identity }: { readonly identity: Identity }) {
  const [failed, setFailed] = useState<string>();
  const source = identity.avatarUrl;
  if (identity.agent === true)
    return (
      <span className="message__avatar" data-agent="" aria-hidden="true">
        <AgentMark />
      </span>
    );
  return (
    <span className="message__avatar" aria-hidden="true">
      {source === undefined || failed === source ? (
        identity.initials
      ) : (
        <img
          src={source}
          alt=""
          width={32}
          height={32}
          decoding="async"
          referrerPolicy="no-referrer"
          onError={() => setFailed(source)}
        />
      )}
    </span>
  );
}
