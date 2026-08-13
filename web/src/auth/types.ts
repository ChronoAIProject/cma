/**
 * Authentication contract.
 *
 * chrono-bot's REST surface documents two client credentials: a NyxID bearer
 * token, or the chrono-bot session cookie for browser clients (docs/API_REST.md).
 * The browser is a cookie client, so nothing here ever holds a raw token — and
 * no implementation may write one to storage.
 *
 * Components depend on this interface only. Swapping the mock adapter for the
 * real NyxID OIDC provider (#3) must require no change to any component.
 */

/** The signed-in subject, as much of it as the frontend is entitled to see. */
export interface User {
  /** Opaque subject identifier. Never a token. */
  readonly id: string;
  /** Display handle, rendered into the shell prompt. */
  readonly handle: string;
  /** Optional avatar URL. Absent means render initials. */
  readonly avatarUrl?: string;
}

/** What the frontend knows about the current identity at any moment. */
export type AuthState =
  | { readonly status: 'checking' }
  | { readonly status: 'anonymous' }
  | { readonly status: 'authenticated'; readonly user: User }
  | { readonly status: 'error'; readonly message: string };

export interface AuthProvider {
  /**
   * Resolve the current identity, typically by validating the session cookie.
   * Implementations must not throw: a failure is reported as an 'error' state
   * so the UI can say what went wrong rather than rendering a blank surface.
   */
  current(): Promise<AuthState>;

  /**
   * Begin sign-in. For the real provider this navigates to NyxID and does not
   * return; the Promise exists so a mock can resolve inline and so callers can
   * await any pre-navigation work.
   */
  signIn(): Promise<void>;

  /** End the session and clear the cookie. */
  signOut(): Promise<void>;
}
