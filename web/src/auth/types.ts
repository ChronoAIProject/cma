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

/**
 * One thing the control plane says this caller may do.
 *
 * These are the core's own capability names, reported by the identity surface
 * (docs/API_REST.md §11). They are used to decide what to *offer*: every one is
 * checked again by the control plane on the request that uses it, so this is
 * never the access control itself. A client that fabricated the list would gain
 * nothing but a control that refuses when pressed.
 *
 * Deliberately not derived from a role name. Roles belong to the identity
 * provider and their names, nesting and wildcards change without this service
 * moving.
 */
export type Capability =
  | 'create_session'
  | 'use_own_sessions'
  | 'terminate_own_session'
  | 'manage_own_environment_profiles'
  | 'read_all_sessions'
  | 'read_runtime_inventory'
  | 'read_event_log'
  | 'read_activity_log'
  | 'read_user_directory'
  | 'control_any_session';

const CAPABILITIES: readonly Capability[] = [
  'create_session',
  'use_own_sessions',
  'terminate_own_session',
  'manage_own_environment_profiles',
  'read_all_sessions',
  'read_runtime_inventory',
  'read_event_log',
  'read_activity_log',
  'read_user_directory',
  'control_any_session',
];

/**
 * Whether a value names a capability this client models.
 *
 * A name it does not know is dropped rather than carried: the frontend has no
 * behaviour to attach to a capability it has never heard of, and keeping the
 * string would only invite a comparison that silently never matches.
 */
export function isCapability(value: unknown): value is Capability {
  return typeof value === 'string' && (CAPABILITIES as readonly string[]).includes(value);
}

/** The signed-in subject, as much of it as the frontend is entitled to see. */
export interface User {
  /**
   * Opaque subject identifier. Never a token, and never shown to a person:
   * it is what state is keyed on, not what someone is called.
   */
  readonly id: string;
  /**
   * What to call this person.
   *
   * Resolved from the provider's account name, then their email, and only
   * then the identifier — which means a bare identifier on screen is a signal
   * the provider released nothing, not a design choice.
   */
  readonly handle: string;
  /** The provider's account name, when it has one. */
  readonly displayName?: string;
  /** The account's email address, when the provider releases it. */
  readonly email?: string;
  /** Optional avatar URL. Absent means render initials. */
  readonly avatarUrl?: string;
  /**
   * What the control plane says this caller may do.
   *
   * Required rather than optional, so every implementation of this port has to
   * say something: an absent field would be read as "unknown" by one caller and
   * "nothing" by another, and only one of those fails closed. An empty list is
   * the honest answer for a caller granted nothing.
   */
  readonly capabilities: readonly Capability[];
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
