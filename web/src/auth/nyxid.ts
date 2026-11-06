/**
 * The real identity provider adapter.
 *
 * Sign-in is a full-page navigation to the control plane, which begins the
 * OIDC exchange and comes back having set an identity cookie. The browser never
 * sees a token, which is the property the auth contract exists to preserve:
 * there is no value here that could be written to storage, so no code path can
 * grow into doing it.
 *
 * The mock this replaces stays in the tree for tests. Swapping between them is
 * a single line in main.tsx, which is what the seam was for.
 */

import type { AuthProvider, AuthState, Capability, SignInOptions } from './types';
import { isCapability } from './types';
import { parseRoute, routePath } from '../routing/routes';

/** Where the control plane answers questions about the current browser identity. */
const IDENTITY_ENDPOINT = '/auth/session';

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

/**
 * Set for the tab once a silent sign-in has been tried. A provider that
 * declines sends the browser back here still anonymous, and asking again would
 * loop; the landing's own sign-in remains for a person who wants it.
 */
const SILENT_ATTEMPT_KEY = 'cma.auth.silent-attempt';

/**
 * Set by signing out of CMA, which ends the CMA cookie but not the provider's
 * session. Without it the next page load would silently sign the person back
 * in. Cleared when they choose to sign in again.
 */
const SIGNED_OUT_KEY = 'cma.auth.signed-out';

/** What the control plane reports for the current cookie. */
interface IdentityResponse {
  authenticated?: unknown;
  subject?: unknown;
  display_name?: unknown;
  email?: unknown;
  avatar_url?: unknown;
  capabilities?: unknown;
  visitor?: unknown;
}

/**
 * What this caller may do, as the control plane reports it.
 *
 * Anything that is not a capability this client models is dropped, and a
 * missing or malformed field yields none at all. Failing closed is the only
 * safe direction: the worst outcome is a control that is not offered, where the
 * alternative is offering one the control plane will refuse.
 */
export function capabilitiesFrom(raw: unknown): readonly Capability[] {
  return Array.isArray(raw) ? raw.filter(isCapability) : [];
}

/**
 * Decide what to call this person.
 *
 * The account name is what they chose to be called, so it wins. An email is
 * the next best thing and is shown as its local part, which is how people
 * refer to each other anyway. The subject identifier is the last resort and
 * means the provider released nothing usable — it is shown rather than hidden
 * so the gap is visible instead of silently rendering an empty label.
 */
export function handleFor(subject: string, displayName?: string, email?: string): string {
  const name = displayName?.trim();
  if (name !== undefined && name !== '') {
    return name;
  }
  const address = email?.trim();
  if (address !== undefined && address !== '') {
    const at = address.indexOf('@');
    return at > 0 ? address.slice(0, at) : address;
  }
  return subject;
}

export function signInPath(returnTo?: string, options?: SignInOptions): string {
  const target = new URL(LOGIN_ENDPOINT, window.location.origin);
  if (returnTo !== undefined) {
    const route = parseRoute(returnTo);
    if (route.kind === 'not_found' || routePath(route) !== returnTo)
      throw new Error('The sign-in destination is not a CMA page.');
    target.searchParams.set('return_to', returnTo);
  }
  if (options?.screen === 'register') target.searchParams.set('screen', 'register');
  return `${target.pathname}${target.search}`;
}

/**
 * Where a silent sign-in returns to, or undefined where none is attempted.
 *
 * Only addresses the control plane accepts as a sign-in return target qualify:
 * a refused target would strand the person on an error rather than the page
 * they opened. Visitor journeys and share links run their own sign-in, which
 * retains the link's authority across the round trip.
 */
export function silentReturnTarget(pathname: string): string | undefined {
  const route = parseRoute(pathname);
  if (routePath(route) !== pathname) return undefined;
  switch (route.kind) {
    case 'home':
      return '/agent-manager';
    case 'agent_manager':
    case 'event_gateway':
    case 'image_manager':
    case 'agent':
    case 'sandbox':
    case 'bots':
    case 'bot_new':
    case 'bot':
    case 'profiles':
    case 'marketplace':
    case 'marketplace_profile':
    case 'triggers':
    case 'trigger':
    case 'trigger_embed':
      return pathname;
    default:
      return undefined;
  }
}

/** Storage can refuse access outright; read that as "do not try". */
function remember(action: () => boolean): boolean {
  try {
    return action();
  } catch {
    return false;
  }
}

/** Read a claim the provider may have omitted. */
function optionalText(value: unknown): string | undefined {
  return typeof value === 'string' && value.trim() !== '' ? value.trim() : undefined;
}

export function createNyxidAuthProvider(): AuthProvider {
  return {
    async current(): Promise<AuthState> {
      let response: Response;
      const timeout = AbortSignal.timeout(IDENTITY_TIMEOUT_MS);
      try {
        response = await fetch(IDENTITY_ENDPOINT, {
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

      let body: IdentityResponse;
      try {
        body = (await response.json()) as IdentityResponse;
      } catch {
        return { status: 'error', message: 'identity response was not readable' };
      }

      if (body.authenticated !== true || typeof body.subject !== 'string' || body.subject === '') {
        // A 200 that does not actually name a subject is treated as signed
        // out, never as an anonymous identity with a blank subject.
        return { status: 'anonymous' };
      }

      remember(() => {
        sessionStorage.removeItem(SILENT_ATTEMPT_KEY);
        return true;
      });
      const displayName = optionalText(body.display_name);
      const email = optionalText(body.email);
      return {
        status: 'authenticated',
        user: {
          id: body.subject,
          handle: handleFor(body.subject, displayName, email),
          displayName,
          email,
          avatarUrl: optionalText(body.avatar_url),
          capabilities: capabilitiesFrom(body.capabilities),
          visitor: body.visitor === true,
        },
      };
    },

    async signIn(returnTo, options): Promise<void> {
      // A navigation, not a fetch: the provider needs to run its own redirects
      // and may show a login page, neither of which survives XHR.
      const path = signInPath(returnTo, options);
      remember(() => {
        localStorage.removeItem(SIGNED_OUT_KEY);
        return true;
      });
      window.location.assign(path);
    },

    async signOut(): Promise<void> {
      // Also a navigation. The control plane clears the cookie and redirects
      // back, so the app reloads already signed out rather than holding stale
      // state it would have to reconcile.
      remember(() => {
        localStorage.setItem(SIGNED_OUT_KEY, '1');
        return true;
      });
      window.location.assign(LOGOUT_ENDPOINT);
    },

    signInSilently(): boolean {
      const returnTo = silentReturnTarget(window.location.pathname);
      if (returnTo === undefined) return false;
      const started = remember(() => {
        if (localStorage.getItem(SIGNED_OUT_KEY) !== null) return false;
        if (sessionStorage.getItem(SILENT_ATTEMPT_KEY) !== null) return false;
        sessionStorage.setItem(SILENT_ATTEMPT_KEY, '1');
        return true;
      });
      if (!started) return false;
      const target = new URL(signInPath(returnTo), window.location.origin);
      target.searchParams.set('prompt', 'none');
      // Replace: the anonymous page was never somewhere the person chose to
      // be, so Back should not return to it.
      window.location.replace(`${target.pathname}${target.search}`);
      return true;
    },
  };
}
