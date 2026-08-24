import { CredentialError, TransportError, ValidationError } from './errors.js';

/** A bearer token resolved for each request and reconnect attempt. */
export type Credential = () => string | undefined | Promise<string | undefined>;

export interface ClientConfig {
  /** Deployment root. Empty means a same-origin deployment. */
  readonly baseUrl?: string;
  /** Bearer token provider. It is called for every attempt so rotation is visible. */
  readonly credential?: Credential;
  /** Whether requests may carry browser cookies. Defaults to true. */
  readonly cookies?: boolean;
  /** Fetch implementation for browsers, Node, tests, and wrapped transports. */
  readonly fetch?: typeof globalThis.fetch;
  /** Default deadline for one request or stream handshake. */
  readonly requestTimeoutMs?: number;
}

export interface ResolvedConfig {
  readonly baseUrl: string;
  readonly credential?: Credential;
  readonly cookies: boolean;
  readonly fetch: typeof globalThis.fetch;
  readonly requestTimeoutMs: number;
}

export const DEFAULT_REQUEST_TIMEOUT_MS = 30_000;

export function resolveConfig(config: ClientConfig = {}): ResolvedConfig {
  const timeout = config.requestTimeoutMs ?? DEFAULT_REQUEST_TIMEOUT_MS;
  if (!Number.isSafeInteger(timeout) || timeout <= 0) {
    throw new ValidationError('requestTimeoutMs must be a positive safe integer');
  }
  return {
    baseUrl: normaliseBaseUrl(config.baseUrl ?? ''),
    credential: config.credential,
    cookies: config.cookies ?? true,
    fetch: config.fetch ?? platformFetch,
    requestTimeoutMs: timeout,
  };
}

function normaliseBaseUrl(value: string): string {
  const base = value.replace(/\/+$/, '');
  if (base === '' || base.startsWith('/')) {
    return base;
  }

  let parsed: URL;
  try {
    parsed = new URL(base);
  } catch (cause) {
    throw new ValidationError('baseUrl must be an HTTP URL or a root-relative path', { cause });
  }
  if (!['http:', 'https:'].includes(parsed.protocol)) {
    throw new ValidationError('baseUrl must use HTTP or HTTPS');
  }
  if (parsed.username !== '' || parsed.password !== '') {
    throw new ValidationError('baseUrl must not contain user information');
  }
  if (parsed.search !== '' || parsed.hash !== '') {
    throw new ValidationError('baseUrl must not contain a query or fragment');
  }
  return base;
}

const platformFetch: typeof globalThis.fetch = (input, init) => {
  const implementation = globalThis.fetch;
  if (typeof implementation !== 'function') {
    throw new TransportError('no fetch is available; inject one through ClientConfig.fetch');
  }
  return implementation.call(globalThis, input, init);
};

export function urlOf(config: ResolvedConfig, path: string): string {
  if (!path.startsWith('/')) {
    throw new ValidationError('request paths must begin with a slash');
  }
  return `${config.baseUrl}${path}`;
}

export async function headersFor(
  config: ResolvedConfig,
  extra?: HeadersInit,
): Promise<Record<string, string>> {
  const headers = new Headers({ 'Content-Type': 'application/json' });
  for (const [key, value] of new Headers(extra ?? {})) {
    headers.set(key, value);
  }
  let token: string | undefined;
  try {
    token = await config.credential?.();
  } catch (cause) {
    throw new CredentialError('the credential provider failed', { cause });
  }
  if (token !== undefined) {
    if (token === '') {
      throw new CredentialError('the credential provider returned an empty bearer token');
    }
    try {
      headers.set('Authorization', `Bearer ${token}`);
    } catch (cause) {
      throw new CredentialError('the credential is not valid in an HTTP bearer header', { cause });
    }
  }
  return Object.fromEntries(headers);
}
