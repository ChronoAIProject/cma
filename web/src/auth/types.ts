/** Authentication state exposed to the retained web shell. */

export type Capability =
  | 'create_sandbox'
  | 'use_own_sandboxes'
  | 'delete_own_sandbox'
  | 'manage_own_bots'
  | 'manage_own_profiles'
  | 'publish_triggers'
  | 'launch_triggers'
  | 'read_all_sandboxes'
  | 'control_any_sandbox'
  | 'read_runtime_inventory'
  | 'read_event_log'
  | 'read_activity_log'
  | 'read_user_directory'
  | 'configure_platform';

const CAPABILITIES: readonly Capability[] = [
  'create_sandbox',
  'use_own_sandboxes',
  'delete_own_sandbox',
  'manage_own_bots',
  'manage_own_profiles',
  'publish_triggers',
  'launch_triggers',
  'read_all_sandboxes',
  'control_any_sandbox',
  'read_runtime_inventory',
  'read_event_log',
  'read_activity_log',
  'read_user_directory',
  'configure_platform',
];

export function isCapability(value: unknown): value is Capability {
  return typeof value === 'string' && (CAPABILITIES as readonly string[]).includes(value);
}

/** The terms and privacy notice this deployment asks owners to accept. */
export interface LegalTerms {
  readonly version: string;
  readonly termsUrl: string;
  readonly privacyUrl: string;
}

export interface User {
  readonly id: string;
  readonly handle: string;
  readonly displayName?: string;
  readonly email?: string;
  readonly avatarUrl?: string;
  readonly capabilities: readonly Capability[];
  readonly visitor: boolean;
  readonly serviceAuthorization?: boolean;
  readonly privacyExport?: boolean;
  readonly bindingDiagnostics?: boolean;
  readonly legalTerms?: LegalTerms;
}

/** Failure disposition only: no response headers, credentials or identity data. */
export type AuthRetryMetadata =
  | { readonly kind: 'transport' }
  | {
      readonly kind: 'http';
      readonly status: 429 | 502 | 503 | 504;
      readonly retryAfterMs?: number;
    };

export type AuthState =
  | { readonly status: 'checking' }
  | { readonly status: 'anonymous'; readonly legalTerms?: LegalTerms }
  | { readonly status: 'authenticated'; readonly user: User }
  | {
      readonly status: 'error';
      readonly message: string;
      readonly reason?: 'identity_unavailable' | 'service_unavailable';
      readonly retry?: AuthRetryMetadata;
    };

export interface AuthProvider {
  current(signal?: AbortSignal): Promise<AuthState>;
  signIn(returnTo?: string, options?: SignInOptions): Promise<void>;
  signOut(): Promise<void>;
  /**
   * Leave to ask the identity provider whether this browser is already signed
   * in, without letting it show anything. Returns false when no attempt was
   * started, and the caller keeps the anonymous state it already has.
   */
  signInSilently?(): boolean;
}

export interface SignInOptions {
  readonly screen?: 'register';
}

/** The published terms known for this identity, signed in or not. */
export function legalTermsOf(state: AuthState): LegalTerms | undefined {
  if (state.status === 'anonymous') return state.legalTerms;
  if (state.status === 'authenticated') return state.user.legalTerms;
  return undefined;
}
