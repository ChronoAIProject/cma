/** Authentication state exposed to the retained web shell. */

export type Capability =
  | 'read_own_resources'
  | 'manage_own_resources'
  | 'read_activity'
  | 'read_directory'
  | 'read_all_resources'
  | 'configure_platform';

const CAPABILITIES: readonly Capability[] = [
  'read_own_resources',
  'manage_own_resources',
  'read_activity',
  'read_directory',
  'read_all_resources',
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
}

export type AuthState =
  | { readonly status: 'checking' }
  | { readonly status: 'anonymous' }
  | { readonly status: 'authenticated'; readonly user: User }
  | { readonly status: 'error'; readonly message: string };

export interface AuthProvider {
  current(): Promise<AuthState>;
  signIn(returnTo?: string): Promise<void>;
  signOut(): Promise<void>;
}
