import type { AuthProvider, AuthState, Capability, User } from './types';

const DAILY_USE: readonly Capability[] = [
  'create_sandbox',
  'use_own_sandboxes',
  'delete_own_sandbox',
  'manage_own_profiles',
  'publish_triggers',
  'launch_triggers',
];

export interface MockAuthOptions {
  readonly initiallyAuthenticated?: boolean;
  readonly failCurrent?: string;
  readonly capabilities?: readonly Capability[];
}

export function createMockAuthProvider(options: MockAuthOptions = {}): AuthProvider {
  const mockUser: User = {
    id: 'mock-subject',
    handle: 'operator',
    capabilities: options.capabilities ?? DAILY_USE,
    visitor: false,
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
      user = mockUser;
    },
    async signOut(): Promise<void> {
      user = null;
    },
  };
}
