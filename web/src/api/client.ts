/**
 * The dashboard's own CMA client.
 *
 * Everything below is `@chronoai/cma-sdk` bound to one configuration: the
 * empty one. That means same-origin paths authenticated by the cookie the
 * browser attaches, which is exactly what this deployment needs — the bundle
 * is served by a proxy that forwards `/api/` and `/auth/` onward, so no host
 * is compiled in and no token is ever handled in the browser. A build that
 * knew the API's address would need rebuilding to move it, and a token in
 * script gives up the one property an http-only cookie has.
 *
 * The binding is the point rather than an implementation detail. `CLAUDE.md`
 * §1 makes this the reference client for the public interfaces, and a
 * reference client that reached the service through its own private code path
 * would demonstrate nothing about the path somebody else has to use. Anything
 * the dashboard needs and the SDK does not export is a gap in the SDK, and it
 * surfaces here as a build failure rather than as a discovery made by whoever
 * integrates next.
 *
 * A second client with a different address or credential is a `createClient`
 * call away; this module is only the one this app happens to hold.
 */

import { createClient, type CreateSession } from '@chronoai/cma-sdk';
import type {
  KernelOffer,
  Session,
  SessionAllowance,
  SessionLifecycle,
  SessionPlane,
  PricingQuote,
  SessionResourceStatus,
  SessionSpend,
} from '@chronoai/cma-sdk';
import { isStage, type Stage } from '../sessions/progress';
import type { SignIn } from '@chronoai/cma-sdk';

export {
  ApiError,
  CapabilityRefused,
  DeploymentAtCapacity,
  SessionLimitReached,
  toSession,
} from '@chronoai/cma-sdk';
export type {
  PricingQuote,
  SessionAllowance,
  SessionResourceStatus,
  SessionSpend,
} from '@chronoai/cma-sdk';

/**
 * The one client this app holds.
 *
 * Same-origin and cookie-authenticated, which is every default — so this
 * configures nothing, and the defaults being right for the reference client is
 * not a coincidence but the reason they are the defaults.
 */
const bot = createClient();

/** Sessions belonging to the signed-in subject, newest first. */
export function listSessions(): Promise<readonly Session[]> {
  return bot.listSessions();
}

/** The signed-in subject's advisory live-session allowance. */
export function readAllowance(): Promise<SessionAllowance> {
  return bot.readAllowance();
}

/**
 * Read one session, as the control plane currently describes it.
 *
 * Used when a session settles: what it was handed at creation is the answer
 * from before provisioning ran, and provisioning is what settles some of it.
 */
export function readSession(id: string): Promise<Session> {
  return bot.readSession(id);
}

/**
 * The runtimes this deployment can start a session on, and what each can be.
 *
 * Asked once and passed down, rather than asked wherever a form needs it: the
 * answer describes the deployment and does not change while somebody fills a
 * form in.
 */
export function listKernels(): Promise<readonly KernelOffer[]> {
  return bot.listKernels();
}

/**
 * Provision a session on the given kernel.
 *
 * Positional here and named in the SDK, deliberately: this is the call the
 * console already makes, and changing thirty call sites to prove a point about
 * argument style would be a worse commit than this adapter.
 */
export function createSession(
  cols: number,
  rows: number,
  environmentProfile?: string | null,
  quotedRateCard?: string,
): Promise<Session> {
  const options: CreateSession = { cols, rows, environmentProfile, quotedRateCard };
  return bot.createSession(options);
}

/** Quote a new session at the deployment's current sealed rate card. */
export function quoteSession(environmentProfile?: string | null): Promise<PricingQuote> {
  return bot.quoteSession(environmentProfile);
}

/** Read reserved-capacity spend derived from one session's durable log. */
export function readSessionSpend(id: string): Promise<SessionSpend> {
  return bot.readSpend(id);
}

/** Read the latest public resource status for one session. */
export function readSessionResources(
  id: string,
  signal?: AbortSignal,
): Promise<SessionResourceStatus> {
  return bot.readResources(id, { signal });
}

/**
 * Move a session onto the other plane.
 *
 * A rotation, and answered as one: the session comes back `rotating`, which is
 * the window in which it takes no input. What it costs — the turn in flight,
 * and the pause — is stated before this is called, never after.
 *
 * A refusal arrives as a `CapabilityRefused` carrying the control plane's own
 * sentence and the name of the missing ability. It is shown rather than
 * flattened: an API that names what is absent is worth nothing if the
 * interface renders it as "something went wrong".
 */
export function changePlane(id: string, plane: SessionPlane): Promise<Session> {
  return bot.changePlane(id, plane);
}

/**
 * End a session, leaving it listed.
 *
 * Distinct from removing it: a finished conversation is still worth reading,
 * and the control plane keeps it until its owner deletes it.
 */
export function terminateSession(id: string): Promise<void> {
  return bot.terminateSession(id);
}

/**
 * Release a session's sandbox, keeping the session.
 *
 * The reversible half of ending something: nothing is running and nothing is
 * being paid for, while the history stays readable and the session can be
 * picked up again with `reviveSession`.
 */
export function stopSession(id: string): Promise<void> {
  return bot.stopSession(id);
}

/** Give a stopped session a sandbox again, under the same id. */
export function reviveSession(id: string): Promise<Session> {
  return bot.reviveSession(id);
}

/**
 * Everything a session's terminal has shown.
 *
 * How a session with no sandbox is read. Bytes rather than text, because it is
 * an escape-sequence stream that only a terminal knows how to interpret.
 */
export function fetchTranscript(id: string): Promise<Uint8Array> {
  return bot.fetchTranscript(id);
}

/** Remove a session and its history permanently. */
export function deleteSession(id: string): Promise<void> {
  return bot.deleteSession(id);
}

const LIFECYCLES = [
  'provisioning',
  'ready',
  'running',
  'rotating',
  'idle',
  'terminated',
] as const satisfies readonly SessionLifecycle[];

function lifecycleFrom(value: unknown): SessionLifecycle | undefined {
  if (typeof value !== 'string' || value === '') {
    return undefined;
  }
  return LIFECYCLES.includes(value as (typeof LIFECYCLES)[number])
    ? (value as (typeof LIFECYCLES)[number])
    : 'unknown';
}

/**
 * Follow a session's events until it stops provisioning.
 *
 * Creating a session returns before its runtime exists, so this is how the UI
 * learns what happened next. Resumes from a cursor and reconnects on its own,
 * which is why it is a stream of server-sent events rather than polling.
 *
 * Returns a cancel function; calling it aborts the active fetch or reconnect.
 */
export function followSession(
  id: string,
  onStatus: (lifecycle: SessionLifecycle, detail?: string) => void,
  onError: (message: string) => void,
  onStage?: (stage: Stage) => void,
  onSignIn?: (signIn: SignIn) => void,
): () => void {
  const controller = new AbortController();

  void (async () => {
    try {
      for await (const frame of bot.streamEvents(id, { signal: controller.signal })) {
        const event = frame.event;

        // A step on the way to ready, reported so the interface can say which
        // part is taking the time rather than only that something is.
        if (event.type === 'progress' && isStage(event.stage)) {
          onStage?.(event.stage);
        }

        /*
         * The agent has stopped to ask its owner to sign in. The SDK has
         * already checked the frame's shape; this projection only turns wire
         * nulls and empty optional strings into UI absence.
         */
        if (event.type === 'sign_in_required' && typeof event.url === 'string') {
          const code = event.code;
          const message = event.message;
          onSignIn?.({
            url: event.url,
            code: typeof code === 'string' && code !== '' ? code : undefined,
            message: typeof message === 'string' && message !== '' ? message : undefined,
          });
        }

        if (event.type === 'status_changed') {
          const lifecycle = lifecycleFrom(event.status);
          if (lifecycle !== undefined) {
            // The detail explains a failure. Without it a session that could
            // not start is indistinguishable from one that ended normally.
            onStatus(lifecycle, typeof event.detail === 'string' ? event.detail : undefined);
          }
        }
      }
    } catch (cause) {
      if (!controller.signal.aborted) {
        onError(cause instanceof Error ? cause.message : String(cause));
      }
    }
  })();

  return () => controller.abort();
}

/** URL of the terminal relay for a session. */
export function terminalUrl(id: string): string {
  const scheme = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  return `${scheme}//${window.location.host}/api/v1/sessions/${encodeURIComponent(id)}/terminal`;
}
