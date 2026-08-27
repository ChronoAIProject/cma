import type { ConnectionAuthorization, TriggerConnection } from '@chronoai/cma-sdk';
import { ApiError } from '../api/client';

const DEFAULT_POLL_MS = 1_500;
const DEFAULT_TIMEOUT_MS = 5 * 60 * 1_000;
const CLOSED_GRACE_MS = 12_000;

export class ConnectionPopupBlocked extends Error {
  constructor() {
    super('The GitHub authorization window was blocked. Allow popups for CMA and try again.');
    this.name = 'ConnectionPopupBlocked';
  }
}

export class ConnectionAuthorizationCancelled extends Error {
  constructor(message = 'GitHub authorization was cancelled before the connection became active.') {
    super(message);
    this.name = 'ConnectionAuthorizationCancelled';
  }
}

export class ConnectionAuthorizationTimedOut extends Error {
  constructor() {
    super(
      'GitHub authorization did not finish within five minutes. Check the connection to continue.',
    );
    this.name = 'ConnectionAuthorizationTimedOut';
  }
}

interface AuthorizeConnectionOptions {
  readonly begin: () => Promise<ConnectionAuthorization>;
  readonly preflight: (serviceId: string, signal: AbortSignal) => Promise<TriggerConnection>;
  readonly onService?: (serviceId: string) => void;
  readonly signal?: AbortSignal;
  readonly open?: typeof window.open;
  readonly now?: () => number;
  readonly pollMs?: number;
  readonly timeoutMs?: number;
}

function githubAuthorizationUrl(value: string): URL {
  let url: URL;
  try {
    url = new URL(value);
  } catch (cause) {
    throw new Error('CMA returned an invalid GitHub authorization URL.', { cause });
  }
  if (
    url.protocol !== 'https:' ||
    url.hostname !== 'github.com' ||
    url.pathname !== '/login/oauth/authorize' ||
    url.username !== '' ||
    url.password !== '' ||
    url.hash !== ''
  ) {
    throw new Error('CMA refused an unexpected GitHub authorization destination.');
  }
  return url;
}

function pending(cause: unknown): boolean {
  return (
    cause instanceof ApiError &&
    (cause.code === 'trigger_connection_inactive' ||
      cause.code === 'trigger_reauthorization_required' ||
      cause.status === 429 ||
      cause.status >= 500)
  );
}

function wait(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal.aborted) {
      reject(new ConnectionAuthorizationCancelled());
      return;
    }
    const onAbort = () => {
      window.clearTimeout(timer);
      reject(new ConnectionAuthorizationCancelled());
    };
    const timer = window.setTimeout(() => {
      signal.removeEventListener('abort', onAbort);
      resolve();
    }, ms);
    signal.addEventListener('abort', onAbort, { once: true });
  });
}

/**
 * Keep CMA in the original window while NyxID completes GitHub OAuth.
 *
 * The popup is created before the first await so this function remains inside
 * the activating click gesture and browsers do not classify it as unsolicited.
 */
export async function authorizeTriggerConnection({
  begin,
  preflight,
  onService,
  signal,
  open = window.open.bind(window),
  now = Date.now,
  pollMs = DEFAULT_POLL_MS,
  timeoutMs = DEFAULT_TIMEOUT_MS,
}: AuthorizeConnectionOptions): Promise<TriggerConnection> {
  const popup = open('about:blank', 'cma-github-authorization', 'popup,width=720,height=760');
  if (popup === null) throw new ConnectionPopupBlocked();

  const controller = new AbortController();
  const abort = () => controller.abort();
  signal?.addEventListener('abort', abort, { once: true });
  const startedAt = now();
  let closedAt: number | undefined;

  try {
    const handoff = await begin();
    if (controller.signal.aborted) throw new ConnectionAuthorizationCancelled();
    onService?.(handoff.service_id);
    popup.location.href = githubAuthorizationUrl(handoff.authorization_url).href;

    while (now() - startedAt < timeoutMs) {
      try {
        const connection = await preflight(handoff.service_id, controller.signal);
        if (connection.service_id !== handoff.service_id) {
          throw new Error('CMA returned a different GitHub connection than the one authorized.');
        }
        if (connection.state === 'active' && connection.account_id !== null) {
          if (!popup.closed) popup.close();
          return connection;
        }
      } catch (cause) {
        if (controller.signal.aborted) throw new ConnectionAuthorizationCancelled();
        if (!pending(cause)) throw cause;
      }

      if (popup.closed) {
        closedAt ??= now();
        if (now() - closedAt >= CLOSED_GRACE_MS) {
          throw new ConnectionAuthorizationCancelled();
        }
      } else {
        closedAt = undefined;
      }
      await wait(pollMs, controller.signal);
    }
    throw new ConnectionAuthorizationTimedOut();
  } catch (cause) {
    if (!popup.closed) popup.close();
    throw cause;
  } finally {
    signal?.removeEventListener('abort', abort);
  }
}

export function isActiveTriggerConnection(
  connection: TriggerConnection | null | undefined,
): connection is TriggerConnection {
  return connection?.state === 'active' && connection.account_id !== null;
}
