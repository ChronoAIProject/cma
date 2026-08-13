import type { AuthProvider, AuthState, User } from './types';

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

const MOCK_USER: User = {
  id: 'mock-subject',
  handle: 'operator',
};

export interface MockAuthOptions {
  /** Start already signed in, for tests and local shell work. */
  readonly initiallyAuthenticated?: boolean;
  /** Force `current()` to report a failure, so error paths can be exercised. */
  readonly failCurrent?: string;
}

export function createMockAuthProvider(options: MockAuthOptions = {}): AuthProvider {
  let user: User | null = options.initiallyAuthenticated === true ? MOCK_USER : null;

  return {
    async current(): Promise<AuthState> {
      if (options.failCurrent !== undefined) {
        return { status: 'error', message: options.failCurrent };
      }
      return user === null ? { status: 'anonymous' } : { status: 'authenticated', user };
    },

    async signIn(): Promise<void> {
      // The real provider navigates to NyxID here and never returns.
      user = MOCK_USER;
    },

    async signOut(): Promise<void> {
      user = null;
    },
  };
}
