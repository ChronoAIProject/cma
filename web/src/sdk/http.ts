/**
 * One request to the control plane, and what comes back.
 *
 * Every call this SDK makes goes through here, so the things that must be true
 * of all of them — the credential, the base address, how a failure is reported
 * — are true in one place rather than at thirty call sites.
 *
 * The wire is a trust boundary. Nothing here trusts a response body's shape:
 * this returns `unknown` and the modules above it validate. A payload coerced
 * into a type it does not have becomes `undefined` somewhere in a surface, far
 * from the request that produced it.
 */

import { headersFor, urlOf, type ResolvedConfig } from './config';
import { ApiError, CapabilityRefused, SessionLimitReached } from './errors';

/** What a caller may vary about one request. */
export interface RequestOptions {
  readonly method?: string;
  readonly body?: unknown;
  readonly headers?: HeadersInit;
  /** Aborts the request. A surface that unmounts should pass one. */
  readonly signal?: AbortSignal;
}

/**
 * Make one request and return its parsed body.
 *
 * `null` for a 204, which is a real answer rather than an empty one: the
 * service says "done, and there is nothing to describe" for a terminate or a
 * delete, and a caller that awaited a body would hang on the parse.
 */
export async function request(
  config: ResolvedConfig,
  path: string,
  options: RequestOptions = {},
): Promise<unknown> {
  let response: Response;
  try {
    response = await config.fetch(urlOf(config, path), {
      method: options.method ?? 'GET',
      headers: await headersFor(config, options.headers),
      // Included by default because the dashboard's credential is a cookie;
      // a cross-origin caller turns this off in its config.
      credentials: config.cookies ? 'include' : 'same-origin',
      body: options.body === undefined ? undefined : JSON.stringify(options.body),
      signal: options.signal,
    });
  } catch (cause) {
    // Status 0 is "nobody answered", which is a different fact from any status
    // the service could have sent, and the one case where retrying is always
    // reasonable.
    throw new ApiError(0, `cannot reach the control plane: ${String(cause)}`);
  }

  if (!response.ok) {
    throw await failureOf(response);
  }
  if (response.status === 204) {
    return null;
  }
  return (await response.json()) as unknown;
}

/**
 * The service's code for "this session was never granted that".
 *
 * Read from the code rather than inferred from the status: 422 is also what a
 * malformed request gets, and those two are the exact pair `CLAUDE.md` §3
 * insists a caller must be able to tell apart — one is worth fixing and
 * retrying, the other never succeeds however many times it is sent.
 */
const CAPABILITY_UNSUPPORTED = 'capability_unsupported';
const SESSION_LIMIT_REACHED = 'session_limit_reached';

/**
 * Turn a failed response into the most specific error it justifies.
 *
 * The service reports failures as an RFC 9457 problem document carrying
 * `detail` and `code`, and — for a refusal — the name of the ability that is
 * missing. That name is the difference between a surface saying "this session
 * cannot hold a conversation" and one saying "something went wrong", so it is
 * read out rather than left inside the sentence.
 */
async function failureOf(response: Response): Promise<ApiError> {
  let detail = `request failed with ${response.status}`;
  let code: string | undefined;
  let capability: string | undefined;
  let limit: number | undefined;
  let live: number | undefined;
  try {
    const body = (await response.json()) as {
      detail?: unknown;
      title?: unknown;
      code?: unknown;
      capability?: unknown;
      limit?: unknown;
      live?: unknown;
    };
    const text = body?.detail ?? body?.title;
    if (typeof text === 'string' && text !== '') {
      detail = text;
    }
    if (typeof body?.code === 'string') {
      code = body.code;
    }
    if (typeof body?.capability === 'string' && body.capability !== '') {
      capability = body.capability;
    }
    if (isNonNegativeInteger(body?.limit)) {
      limit = body.limit;
    }
    if (isNonNegativeInteger(body?.live)) {
      live = body.live;
    }
  } catch {
    // A body that is not a problem document tells us nothing the status line
    // did not already, and failing to parse it is not itself worth reporting.
  }
  if (code === CAPABILITY_UNSUPPORTED) {
    return new CapabilityRefused(response.status, detail, capability);
  }
  if (code === SESSION_LIMIT_REACHED) {
    return new SessionLimitReached(response.status, detail, limit, live);
  }
  return new ApiError(response.status, detail);
}

function isNonNegativeInteger(value: unknown): value is number {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0;
}
