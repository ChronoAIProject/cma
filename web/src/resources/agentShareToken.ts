import { routePath } from '../routing/routes';

const prefix = 'cma:agent-share:';
const lifetime = 30 * 60 * 1000;
const valid = (value: string) => /^[a-f0-9]{64}$/.test(value);

export function newAgentShareToken(): string {
  return Array.from(crypto.getRandomValues(new Uint8Array(32)), (byte) =>
    byte.toString(16).padStart(2, '0'),
  ).join('');
}

export function agentShareUrl(id: string, token: string): string {
  if (!valid(token)) throw new Error('The share link could not be prepared.');
  return `${window.location.origin}${routePath({ kind: 'agent_share', shareId: id })}#${token}`;
}

/** Reading does not change the URL, including React's repeated initial render. */
export function readAgentShareToken(id: string): string | undefined {
  const fragment = window.location.hash.slice(1);
  if (valid(fragment)) return fragment;
  try {
    const stored: unknown = JSON.parse(sessionStorage.getItem(prefix + id) ?? 'null');
    if (
      stored &&
      typeof stored === 'object' &&
      'token' in stored &&
      typeof stored.token === 'string' &&
      valid(stored.token) &&
      'expiresAt' in stored &&
      typeof stored.expiresAt === 'number' &&
      stored.expiresAt > Date.now()
    )
      return stored.token;
  } catch {
    /* No retained authority. */
  }
  return undefined;
}

/** Keep the bearer fragment out of OAuth URLs, HTTP requests and browser history. */
export function retainAgentShareToken(id: string, requireStorage = false): string | undefined {
  const token = window.location.hash.slice(1);
  if (valid(token)) {
    try {
      sessionStorage.setItem(
        prefix + id,
        JSON.stringify({ token, expiresAt: Date.now() + lifetime }),
      );
    } catch {
      if (requireStorage) {
        throw new Error('Allow session storage, then reopen the share link to sign in.');
      }
    }
    window.history.replaceState(
      window.history.state,
      '',
      window.location.pathname + window.location.search,
    );
    return token;
  }
  const retained = readAgentShareToken(id);
  if (!retained) clearAgentShareToken(id);
  return retained;
}

export function clearAgentShareToken(id: string): void {
  try {
    sessionStorage.removeItem(prefix + id);
  } catch {
    /* Storage may be disabled. */
  }
}

export function retainAgentShareForSignIn(id: string): void {
  if (!retainAgentShareToken(id, true)) {
    throw new Error('Open the original share link before signing in.');
  }
}
