import { useState } from 'react';
import { AgentMascot } from '../brand/AgentMascot';
import type { User } from '../auth/types';

export type MessageUser = Pick<User, 'displayName' | 'handle' | 'avatarUrl'>;

interface Identity {
  readonly name: string;
  readonly initials: string;
  readonly avatarUrl?: string;
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

export function MessageAvatar({
  identity,
  continued = false,
}: {
  readonly identity: Identity;
  /** Keeps the avatar column so a continuation's text aligns with the run's first message. */
  readonly continued?: boolean;
}) {
  const [failed, setFailed] = useState<string>();
  const source = identity.avatarUrl;
  if (continued) return <span className="message__avatar" data-continued="" aria-hidden="true" />;
  if (identity.agent === true)
    return (
      <span className="message__avatar" data-agent="" aria-hidden="true">
        <AgentMascot visual={{ action: 'ready', label: identity.name }} size={32} decorative />
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
