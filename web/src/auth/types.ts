/** Authentication state exposed to the retained web shell. */

export type Capability =
  | 'create_sandbox'
  | 'use_own_sandboxes'
  | 'delete_own_sandbox'
  | 'delete_own_account'
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
  'delete_own_account',
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

export interface User {
  readonly id: string;
  readonly handle: string;
  readonly displayName?: string;
  readonly email?: string;
  readonly avatarUrl?: string;
  readonly capabilities: readonly Capability[];
  readonly visitor: boolean;
}

export type AuthState =
  | { readonly status: 'checking' }
  | { readonly status: 'anonymous' }
  | { readonly status: 'authenticated'; readonly user: User }
  | {
      readonly status: 'error';
      readonly message: string;
      readonly reason?: 'identity_unavailable' | 'service_unavailable';
    };

export interface AuthProvider {
  current(): Promise<AuthState>;
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
