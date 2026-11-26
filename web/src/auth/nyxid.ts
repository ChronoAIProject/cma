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

import type { AuthProvider, AuthState, Capability, LegalTerms, SignInOptions } from './types';
import { isCapability } from './types';
import { canonicalReturnRoute, classifyReturnTarget } from './returnTarget';
import { authHttpRetry, authTransportRetry } from './retryMetadata';
import { MAX_LOGIN_URL_BYTES, retryAfterMilliseconds } from '@cma/client';
import { retainFatherDraftForSignIn } from '../bots/fatherDraft';
import { publishSignOutNotice } from './signOutNotice';

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

/** What the control plane reports for the current cookie. */
interface IdentityResponse {
  authenticated?: unknown;
  subject?: unknown;
  display_name?: unknown;
  email?: unknown;
  avatar_url?: unknown;
  capabilities?: unknown;
  visitor?: unknown;
  silent_sign_in?: unknown;
  service_authorization?: unknown;
  privacy_export?: unknown;
  legal_terms?: unknown;
  binding_diagnostics?: unknown;
}

/** Published terms, or none when the field is absent or not exactly usable. */
export function legalTermsFrom(raw: unknown): LegalTerms | undefined {
  if (typeof raw !== 'object' || raw === null) return undefined;
  const { version, terms_url: termsUrl, privacy_url: privacyUrl } = raw as Record<string, unknown>;
  if (typeof version !== 'string' || !/^[A-Za-z0-9._-]{1,64}$/.test(version)) return undefined;
  if (!isHttpsUrl(termsUrl) || !isHttpsUrl(privacyUrl)) return undefined;
  return { version, termsUrl, privacyUrl };
}

function withLegalTerms(legalTerms: LegalTerms | undefined): { legalTerms?: LegalTerms } {
  return legalTerms === undefined ? {} : { legalTerms };
}

function isHttpsUrl(value: unknown): value is string {
  if (typeof value !== 'string') return false;
  try {
    return new URL(value).protocol === 'https:';
  } catch {
    return false;
  }
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

function loginTarget(returnTo?: string, options?: SignInOptions): URL {
  const target = new URL(LOGIN_ENDPOINT, window.location.origin);
  if (returnTo !== undefined) {
    const destination = classifyReturnTarget(returnTo);
    if (destination.kind === 'reject')
      throw new Error('The sign-in destination is not a CMA page.');
    target.searchParams.set('return_to', destination.target);
  }
  if (options?.screen === 'register') target.searchParams.set('screen', 'register');
  return target;
}

export function signInPath(returnTo?: string, options?: SignInOptions): string {
  let target = loginTarget(returnTo, options);
  if (target.href.length > MAX_LOGIN_URL_BYTES)
    target = loginTarget(returnTo?.split('?')[0], options);
  if (target.href.length > MAX_LOGIN_URL_BYTES) throw new Error('The sign-in address is too long.');
  return `${target.pathname}${target.search}`;
}

/**
 * Where a silent sign-in returns to, or undefined where none is attempted.
 *
 * Only addresses the control plane accepts as a sign-in return target qualify:
 * a refused target would strand the person on an error rather than the page
 * they opened. Only defined, non-authorizing setup/tab query state is restored. Fragments
 * stay in the browser because they may carry share credentials.
 * Visitor journeys and share links run their own sign-in, which retains the
 * link's authority across the round trip.
 */
export function silentReturnTarget(location: {
  readonly pathname: string;
  readonly search: string;
  readonly hash: string;
}): string | undefined {
  const { pathname } = location;
  if (location.hash !== '') return undefined;
  const route = canonicalReturnRoute(pathname + location.search);
  if (route === undefined) return undefined;
  switch (route.kind) {
    case 'home':
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
    case 'trigger_embed': {
      const target = loginTarget(pathname + location.search);
      target.searchParams.set('prompt', 'none');
      return target.href.length <= MAX_LOGIN_URL_BYTES ? pathname + location.search : undefined;
    }
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
  // Whether the control plane last said it implements silent sign-in. An older
  // one would refuse the request, so the client never assumes it.
  let silentSignInOffered = false;
  return {
    async current(signal?: AbortSignal): Promise<AuthState> {
      silentSignInOffered = false;
      let response: Response;
      const timeout = AbortSignal.timeout(IDENTITY_TIMEOUT_MS);
      try {
        response = await fetch(IDENTITY_ENDPOINT, {
          credentials: 'include',
          cache: 'no-store',
          signal: signal === undefined ? timeout : AbortSignal.any([signal, timeout]),
        });
      } catch (error) {
        // Unreachable is not anonymous. Saying "signed out" here would invite
        // a sign-in attempt that cannot succeed either.
        const timedOut = timeout.aborted;
        return {
          status: 'error',
          reason: 'service_unavailable',
          retry: authTransportRetry(error, signal, timedOut),
          message: timedOut
            ? 'CMA did not respond. Retrying shortly.'
            : 'Cannot reach CMA. Check your connection; retrying shortly.',
        };
      }

      if (response.status === 401 || response.status === 403) {
        let legalTerms: LegalTerms | undefined;
        try {
          const anonymous = (await response.json()) as IdentityResponse;
          silentSignInOffered =
            response.status === 401 &&
            anonymous?.authenticated === false &&
            anonymous.silent_sign_in === true;
          legalTerms = legalTermsFrom(anonymous?.legal_terms);
        } catch {
          silentSignInOffered = false;
        }
        return legalTerms === undefined
          ? { status: 'anonymous' }
          : { status: 'anonymous', legalTerms };
      }
      if (!response.ok) {
        const problem = (await response.json().catch(() => undefined)) as
          { code?: unknown } | undefined;
        const retry = authHttpRetry(response.status, problem, retryAfterMilliseconds(response));
        if (response.status === 503 && problem?.code === 'identity_unavailable') {
          return {
            status: 'error',
            reason: 'identity_unavailable',
            retry,
            message: 'The identity service is temporarily unavailable. Retrying shortly.',
          };
        }
        return {
          status: 'error',
          retry,
          // Auth-store failures include upstream limits and malformed evidence.
          // Do not turn those into automatic browser retries of a NyxID 429.
          reason:
            [502, 503, 504].includes(response.status) && problem?.code !== 'store_failed'
              ? 'service_unavailable'
              : undefined,
          message: `CMA could not verify your sign-in (HTTP ${response.status}).`,
        };
      }

      let body: IdentityResponse;
      try {
        body = (await response.json()) as IdentityResponse;
      } catch (error) {
        return {
          status: 'error',
          message: 'identity response was not readable',
          retry: authTransportRetry(error, signal, timeout.aborted),
        };
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
          ...(body.service_authorization === true ? { serviceAuthorization: true } : {}),
          ...(body.privacy_export === true ? { privacyExport: true } : {}),
          ...(body.binding_diagnostics === true ? { bindingDiagnostics: true } : {}),
          ...withLegalTerms(legalTermsFrom(body.legal_terms)),
        },
      };
    },

    async signIn(returnTo, options): Promise<void> {
      // A navigation, not a fetch: the provider needs to run its own redirects
      // and may show a login page, neither of which survives XHR.
      const target = signInPath(returnTo, options);
      retainFatherDraftForSignIn();
      window.location.assign(target);
    },

    async signOut(): Promise<void> {
      const request: RequestInit = {
        credentials: 'same-origin',
        mode: 'same-origin',
        cache: 'no-store',
        redirect: 'manual',
        signal: AbortSignal.timeout(IDENTITY_TIMEOUT_MS),
      };
      const failed = new Error('Sign-out did not complete. Please try again.');
      const response = await fetch(LOGOUT_ENDPOINT, request);
      if (response.type !== 'opaqueredirect') throw failed;
      // Fetch hides the redirect status. Confirm the cleared session before requesting the landing page.
      const identity = await fetch(IDENTITY_ENDPOINT, request);
      if (identity.status !== 401) throw failed;
      const body: IdentityResponse | null = await identity.json().catch(() => null);
      if (body?.authenticated !== false || body.silent_sign_in === true) throw failed;
      remember(() => {
        sessionStorage.setItem(SILENT_ATTEMPT_KEY, '1');
        return true;
      });
      publishSignOutNotice();
      window.location.assign('/');
    },

    signInSilently(): boolean {
      if (!silentSignInOffered) return false;
      const returnTo = silentReturnTarget(window.location);
      if (returnTo === undefined) {
        // An oversized setup stays at its original URL and in tab storage.
        remember(() => {
          retainFatherDraftForSignIn();
          return true;
        });
        return false;
      }
      const started = remember(() => {
        if (sessionStorage.getItem(SILENT_ATTEMPT_KEY) !== null) return false;
        sessionStorage.setItem(SILENT_ATTEMPT_KEY, '1');
        return true;
      });
      if (!started) return false;
      const target = new URL(signInPath(returnTo), window.location.origin);
      target.searchParams.set('prompt', 'none');
      // Replace: the anonymous page was never somewhere the person chose to
      // be, so Back should not return to it.
      try {
        window.location.replace(`${target.pathname}${target.search}`);
        return true;
      } catch {
        return false;
      }
    },
  };
}
