/**
 * The real identity provider adapter.
 *
 * Sign-in is a full-page navigation to the control plane, which begins the
 * OIDC exchange and comes back having set a session cookie. The browser never
 * sees a token, which is the property the auth contract exists to preserve:
 * there is no value here that could be written to storage, so no code path can
 * grow into doing it.
 *
 * The mock this replaces stays in the tree for tests. Swapping between them is
 * a single line in main.tsx, which is what the seam was for.
 */

import type { AuthProvider, AuthState } from './types';

/** Where the control plane answers questions about the current session. */
const SESSION_ENDPOINT = '/auth/session';

/**
 * How long the identity check may take before it is called a failure.
 *
 * The app renders nothing while the check is outstanding, on the reasoning
 * that it resolves in milliseconds against a cookie. When the control plane is
 * unreachable that reasoning inverts: the request hangs, and the user is left
 * looking at a blank page with nothing to act on. A bounded wait turns that
 * into a message.
 */
const IDENTITY_TIMEOUT_MS = 8000;
const LOGIN_ENDPOINT = '/auth/login';
const LOGOUT_ENDPOINT = '/auth/logout';

/** What the control plane reports for the current cookie. */
interface SessionResponse {
  authenticated?: unknown;
  subject?: unknown;
}

/**
 * Render a subject identifier as something worth showing a person.
 *
 * The control plane returns an opaque subject, which is correct — it is the
 * frontend's job to present it. An address is shown as its local part; anything
 * else is shown as-is rather than truncated into something unrecognisable.
 */
export function handleFor(subject: string): string {
  const at = subject.indexOf('@');
  return at > 0 ? subject.slice(0, at) : subject;
}

export function createNyxidAuthProvider(): AuthProvider {
  return {
    async current(): Promise<AuthState> {
      let response: Response;
      const timeout = AbortSignal.timeout(IDENTITY_TIMEOUT_MS);
      try {
        response = await fetch(SESSION_ENDPOINT, {
          credentials: 'include',
          signal: timeout,
        });
      } catch (cause) {
        // Unreachable is not anonymous. Saying "signed out" here would invite
        // a sign-in attempt that cannot succeed either.
        const timedOut = timeout.aborted;
        return {
          status: 'error',
          message: timedOut
            ? 'The control plane did not respond. It may be starting up — reload in a moment.'
            : `cannot reach the control plane: ${String(cause)}`,
        };
      }

      if (response.status === 401) {
        return { status: 'anonymous' };
      }
      if (!response.ok) {
        return { status: 'error', message: `identity check failed with ${response.status}` };
      }

      let body: SessionResponse;
      try {
        body = (await response.json()) as SessionResponse;
      } catch {
        return { status: 'error', message: 'identity response was not readable' };
      }

      if (body.authenticated !== true || typeof body.subject !== 'string' || body.subject === '') {
        // A 200 that does not actually name a subject is treated as signed
        // out, never as an anonymous session with a blank identity.
        return { status: 'anonymous' };
      }

      return {
        status: 'authenticated',
        user: { id: body.subject, handle: handleFor(body.subject) },
      };
    },

    async signIn(): Promise<void> {
      // A navigation, not a fetch: the provider needs to run its own redirects
      // and may show a login page, neither of which survives XHR.
      window.location.assign(LOGIN_ENDPOINT);
    },

    async signOut(): Promise<void> {
      // Also a navigation. The control plane clears the cookie and redirects
      // back, so the app reloads already signed out rather than holding stale
      // state it would have to reconcile.
      window.location.assign(LOGOUT_ENDPOINT);
    },
  };
}
