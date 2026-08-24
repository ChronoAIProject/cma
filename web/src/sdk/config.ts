/**
 * How a client is pointed at a CMA deployment.
 *
 * This is the whole reason the SDK exists as something separable from `web/`.
 * The code it was extracted from reaches the service one way and one way only:
 * a same-origin path, authenticated by a cookie the browser attaches. That is
 * correct for the dashboard — it is served by a proxy that forwards `/api/`
 * onward, so no host is compiled in and no token is ever handled in the
 * browser — and it is unusable for anybody else. A product frontend on its own
 * domain, a backend calling from Node, a test driving the real service: none
 * of them are same-origin, and none of them have that cookie.
 *
 * So the address and the credential are given rather than assumed, and the
 * defaults are exactly what the dashboard already does. `web/` configures
 * nothing and behaves as it did; a third party configures both and needs no
 * fork.
 *
 * `CLAUDE.md` §1 puts it as the standard this client is held to: CMA's
 * consumers are other products, the dashboard is the reference client, and the
 * exit criterion for the public interfaces is that somebody else can build on
 * them. A reference client that could only ever run in one place would prove
 * nothing about the interfaces it is supposed to be demonstrating.
 */

/**
 * What a request needs in order to be allowed.
 *
 * Resolved per request rather than captured once: an access token expires
 * mid-session, and a client that read one at construction would work until it
 * suddenly did not. Returning `undefined` means "send no credential", which is
 * what the cookie transport wants — the browser attaches it and this SDK never
 * sees it.
 */
export type Credential = () => string | undefined | Promise<string | undefined>;

/** Where a CMA deployment is and how to be allowed in. */
export interface ClientConfig {
  /**
   * The deployment's base address, with no trailing slash.
   *
   * Empty by default, which yields same-origin paths — the dashboard's case,
   * where a proxy in front of the bundle forwards onward. A build that knew
   * the API's address would have to be rebuilt to move it.
   */
  readonly baseUrl?: string;
  /**
   * The bearer token to present, if this caller presents one.
   *
   * Absent by default. The dashboard sends no token deliberately: its
   * credential is an http-only cookie, which is the one form a browser cannot
   * leak to script, and handling a token there would give up that property.
   */
  readonly credential?: Credential;
  /**
   * Whether to let the browser attach its cookies.
   *
   * On by default, because that is the dashboard's credential. A cross-origin
   * caller should turn it off: sending cookies to another origin either fails
   * CORS preflight or, worse, quietly succeeds and authenticates as somebody
   * the caller did not intend.
   */
  readonly cookies?: boolean;
  /**
   * The `fetch` to use.
   *
   * Injected so this runs off the browser global — a Node client, a test with
   * no network, a caller that wraps every request in a retry. Defaults to the
   * platform's own.
   */
  readonly fetch?: typeof globalThis.fetch;
}

/** A config with every default filled in, which is what the SDK passes around. */
export interface ResolvedConfig {
  readonly baseUrl: string;
  readonly credential?: Credential;
  readonly cookies: boolean;
  readonly fetch: typeof globalThis.fetch;
}

/**
 * Fill in what the caller left out.
 *
 * A trailing slash is stripped rather than tolerated: every path in this SDK
 * begins with one, and `https://host/` joined to `/api/v1` is `https://host//api/v1`,
 * which some gateways route and others 404 — a difference that shows up in
 * production and not in a test.
 */
export function resolveConfig(config: ClientConfig = {}): ResolvedConfig {
  return {
    baseUrl: (config.baseUrl ?? '').replace(/\/+$/, ''),
    credential: config.credential,
    cookies: config.cookies ?? true,
    fetch: config.fetch === undefined ? platformFetch : config.fetch,
  };
}

/**
 * The platform's own `fetch`, looked up when it is called rather than now.
 *
 * Captured at construction it would be whatever existed at import time, and a
 * client is usually constructed at module scope — so a `fetch` installed
 * afterwards was ignored. That breaks a consumer who wraps it for retries or
 * tracing, and it broke every test that stubs the global, which is how the
 * problem was found.
 *
 * Bound to `globalThis` at the call, because an unbound `fetch` invoked as a
 * bare function throws "Illegal invocation" in a browser — exactly how this
 * SDK invokes it.
 */
const platformFetch: typeof globalThis.fetch = (input, init) => {
  const impl = globalThis.fetch;
  if (typeof impl !== 'function') {
    throw new TypeError(
      'no fetch is available: pass one as `fetch` in the config, or run somewhere that has it',
    );
  }
  return impl.call(globalThis, input, init);
};

/** The absolute address of a path under this deployment. */
export function urlOf(config: ResolvedConfig, path: string): string {
  return `${config.baseUrl}${path}`;
}

/**
 * The headers a request carries, with the credential resolved.
 *
 * `Authorization` is set only when a credential was configured and returned
 * something. An empty token is treated as no token rather than sent: `Bearer `
 * with nothing after it reads as a malformed credential and is refused with a
 * 401 that looks exactly like an expired one, which sends whoever is debugging
 * it to the wrong place.
 *
 * Names come back lowercased, because everything goes through `Headers` to get
 * there. That is not cosmetic: header names are case-insensitive, and building
 * the object by hand meant a caller passing `content-type` got it *added*
 * beside the default `Content-Type` rather than replacing it, leaving the
 * request carrying two and the service picking one.
 */
export async function headersFor(
  config: ResolvedConfig,
  extra?: HeadersInit,
): Promise<Record<string, string>> {
  const headers = new Headers({ 'Content-Type': 'application/json' });
  const token = await config.credential?.();
  if (token !== undefined && token !== '') {
    headers.set('Authorization', `Bearer ${token}`);
  }
  // `set`, so a caller's version replaces the default rather than joining it.
  for (const [key, value] of new Headers(extra ?? {})) {
    headers.set(key, value);
  }
  return Object.fromEntries(headers);
}
