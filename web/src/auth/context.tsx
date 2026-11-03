import { createContext, useCallback, useContext, useEffect, useMemo, useState } from 'react';
import type { ReactNode } from 'react';
import type { AuthProvider, AuthState, SignInOptions } from './types';
import { parseRoute, routePath } from '../routing/routes';
import { retainDocumentShareToken } from '../documents/shareToken';
import { retainAgentShareForSignIn } from '../resources/agentShareToken';
import { retainTaskViewer } from '../create/taskIdentity';

/**
 * Wires an AuthProvider into React. Components consume `useAuth()` and never
 * reference a concrete provider, so the mock and the real NyxID adapter are
 * interchangeable without touching a single consumer.
 */

interface AuthContextValue {
  readonly state: AuthState;
  readonly signIn: (returnTo?: string, options?: SignInOptions) => Promise<void>;
  readonly signOut: () => Promise<void>;
}

const AuthContext = createContext<AuthContextValue | null>(null);

interface AuthProviderProps {
  readonly provider: AuthProvider;
  readonly children: ReactNode;
}

export function AuthContextProvider({ provider, children }: AuthProviderProps) {
  const [state, setState] = useState<AuthState>({ status: 'checking' });

  useEffect(() => {
    // Guards against a resolved promise landing after unmount, which React 19
    // StrictMode makes routine by running effects twice in development.
    let live = true;

    provider
      .current()
      .then((next) => {
        if (live) {
          if (next.status === 'authenticated') retainTaskViewer(next.user.id);
          if (next.status === 'anonymous') retainTaskViewer();
          setState(next);
        }
      })
      .catch((error: unknown) => {
        // The contract says implementations must not throw, but a provider is
        // injected and may misbehave. Surface it rather than hanging on
        // 'checking' forever, which would render an empty page with no reason.
        if (live) {
          setState({
            status: 'error',
            message: error instanceof Error ? error.message : 'Could not verify your identity.',
          });
        }
      });

    return () => {
      live = false;
    };
  }, [provider]);

  const signIn = useCallback(
    async (returnTo?: string, options?: SignInOptions) => {
      try {
        const current = parseRoute(window.location.pathname);
        if (current.kind === 'agent_share') {
          retainAgentShareForSignIn(current.shareId);
          returnTo = routePath(current);
        }
        if (current.kind === 'document') {
          retainDocumentShareToken(current.documentId, true);
          returnTo = routePath(current);
        }
        await provider.signIn(returnTo, options);
        const next = await provider.current();
        if (next.status === 'authenticated') retainTaskViewer(next.user.id);
        if (next.status === 'anonymous') retainTaskViewer();
        setState(next);
      } catch (error: unknown) {
        setState({
          status: 'error',
          message: error instanceof Error ? error.message : 'Sign-in could not be started.',
        });
      }
    },
    [provider],
  );

  const signOut = useCallback(async () => {
    try {
      await provider.signOut();
      retainTaskViewer();
      setState({ status: 'anonymous' });
    } catch (error: unknown) {
      setState({
        status: 'error',
        message: error instanceof Error ? error.message : 'Sign-out did not complete.',
      });
    }
  }, [provider]);

  const value = useMemo<AuthContextValue>(
    () => ({ state, signIn, signOut }),
    [state, signIn, signOut],
  );

  return <AuthContext value={value}>{children}</AuthContext>;
}

export function useAuth(): AuthContextValue {
  const value = useContext(AuthContext);
  if (value === null) {
    throw new Error('useAuth must be used within an AuthContextProvider');
  }
  return value;
}
