/**
 * The dashboard's own chrono-bot client.
 *
 * Everything below is the SDK (`web/src/sdk/`) bound to one configuration: the
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

import { createClient, type CreateSession } from '../sdk';
import type { KernelOffer, Session, SessionLifecycle, SessionPlane } from '../sdk';
import { isStage, type Stage } from '../sessions/progress';
import type { SignIn } from '../chat/conversationModel';

export { ApiError, CapabilityRefused, toSession } from '../sdk';

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
): Promise<Session> {
  const options: CreateSession = { cols, rows, environmentProfile };
  return bot.createSession(options);
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

const LIFECYCLES: readonly SessionLifecycle[] = [
  'provisioning',
  'ready',
  'running',
  'rotating',
  'idle',
  'terminated',
];

/**
 * Follow a session's events until it stops provisioning.
 *
 * Creating a session returns before its runtime exists, so this is how the UI
 * learns what happened next. Resumes from a cursor and reconnects on its own,
 * which is why it is a stream of server-sent events rather than polling.
 *
 * Not in the SDK yet, and the reason is worth stating: `EventSource` cannot
 * carry an `Authorization` header, so this works only for a caller whose
 * credential is a cookie. Moving it as it stands would put an interface in the
 * SDK that silently fails for every cross-origin consumer, which is worse than
 * its absence — `CLAUDE.md` §9: declare what cannot be done rather than
 * offering it and dropping the request.
 *
 * Returns a cancel function; calling it closes the stream.
 */
export function followSession(
  id: string,
  onStatus: (lifecycle: SessionLifecycle, detail?: string) => void,
  onError: (message: string) => void,
  onStage?: (stage: Stage) => void,
  onSignIn?: (signIn: SignIn) => void,
): () => void {
  const source = new EventSource(`/api/v1/sessions/${encodeURIComponent(id)}/events`, {
    withCredentials: true,
  });

  source.onmessage = (message: MessageEvent) => {
    let payload: {
      event?: {
        type?: string;
        status?: string;
        detail?: string;
        stage?: string;
        url?: string;
        code?: string;
        message?: string;
      };
    };
    try {
      payload = JSON.parse(String(message.data)) as typeof payload;
    } catch {
      // A frame we cannot read is not worth tearing the stream down for; the
      // next one carries the same state.
      return;
    }
    // A step on the way to ready, reported so the interface can say which
    // part is taking the time rather than only that something is.
    const stage = payload.event?.stage;
    if (isStage(stage)) {
      onStage?.(stage);
    }

    /*
     * The agent has stopped to ask its owner to sign in. Raised from inside
     * the conversation open, so it always arrives while the session is still
     * provisioning — which is the only reason this stream carries it at all,
     * since that is the one state whose surface is drawn from here rather
     * than from the conversation.
     *
     * The address is the whole of the request; a kernel that gave no code is
     * still actionable, so only the address is required.
     */
    if (payload.event?.type === 'sign_in_required') {
      const url = payload.event.url;
      if (typeof url === 'string' && url !== '') {
        /*
         * Read defensively, the same way the AG-UI fold does. The wire sends
         * `null` for a code the kernel did not declare separately — Codex puts
         * it inside the sentence instead — and `null` is not `undefined`, so
         * passing it through drew an empty code block above a Copy control
         * that copied nothing. Absent has to arrive as absent.
         */
        const code = payload.event.code;
        const message = payload.event.message;
        onSignIn?.({
          url,
          code: typeof code === 'string' && code !== '' ? code : undefined,
          message: typeof message === 'string' && message !== '' ? message : undefined,
        });
      }
    }

    const status = payload.event?.status;
    if (typeof status === 'string' && LIFECYCLES.includes(status as SessionLifecycle)) {
      // The detail explains a failure. Without it a session that could not
      // start is indistinguishable from one that ended normally.
      onStatus(status as SessionLifecycle, payload.event?.detail);
    }
  };

  source.onerror = () => {
    // EventSource reconnects by itself, so this is only worth reporting if the
    // browser has given up entirely.
    if (source.readyState === EventSource.CLOSED) {
      onError('lost contact with the control plane while starting the session');
    }
  };

  return () => source.close();
}

/** URL of the terminal relay for a session. */
export function terminalUrl(id: string): string {
  const scheme = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  return `${scheme}//${window.location.host}/api/v1/sessions/${encodeURIComponent(id)}/terminal`;
}
