import type { AuthProvider, AuthState, Capability, User } from './types';

/**
 * Stand-in identity provider.
 *
 * The control plane has no auth endpoint yet — NyxID OIDC with PKCE is
 * specified in #3 but unimplemented — so the shell is developed against this.
 * It exists to prove the seam: when the real provider lands it replaces this
 * module and no component changes.
 *
 * It deliberately holds identity in memory only. Nothing is written to
 * localStorage or sessionStorage, so there is no path by which this could grow
 * into storing a credential.
 */

/**
 * What a signed-in person may do here by default.
 *
 * The everyday set and nothing more, mirroring what the control plane grants a
 * caller who has not been made an operator. A mock that handed out the operator
 * capabilities would make the surface that is meant to be hidden the one every
 * test sees.
 */
const DAILY_USE: readonly Capability[] = [
  'create_session',
  'use_own_sessions',
  'terminate_own_session',
  'manage_own_environment_profiles',
];

export interface MockAuthOptions {
  /** Start already signed in, for tests and local shell work. */
  readonly initiallyAuthenticated?: boolean;
  /** Force `current()` to report a failure, so error paths can be exercised. */
  readonly failCurrent?: string;
  /** What the signed-in person may do. Defaults to the everyday set. */
  readonly capabilities?: readonly Capability[];
}

export function createMockAuthProvider(options: MockAuthOptions = {}): AuthProvider {
  const mockUser: User = {
    id: 'mock-subject',
    handle: 'operator',
    capabilities: options.capabilities ?? DAILY_USE,
  };
  let user: User | null = options.initiallyAuthenticated === true ? mockUser : null;

  return {
    async current(): Promise<AuthState> {
      if (options.failCurrent !== undefined) {
        return { status: 'error', message: options.failCurrent };
      }
      return user === null ? { status: 'anonymous' } : { status: 'authenticated', user };
    },

    async signIn(): Promise<void> {
      // The real provider navigates to NyxID here and never returns.
      user = mockUser;
    },

    async signOut(): Promise<void> {
      user = null;
    },
  };
}
