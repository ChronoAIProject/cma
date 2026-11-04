import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
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
  const currentRead = useRef(0);

  useEffect(() => {
    // Guards against a resolved promise landing after unmount, which React 19
    // StrictMode makes routine by running effects twice in development.
    let live = true;

    const refresh = () => {
      const version = ++currentRead.current;
      void provider
        .current()
        .then((next) => {
          if (live && version === currentRead.current) {
            if (next.status === 'authenticated') retainTaskViewer(next.user.id);
            if (next.status === 'anonymous') retainTaskViewer();
            setState(next);
          }
        })
        .catch((error: unknown) => {
          // The contract says implementations must not throw, but a provider is
          // injected and may misbehave. Surface it rather than hanging on
          // 'checking' forever, which would render an empty page with no reason.
          if (live && version === currentRead.current) {
            setState({
              status: 'error',
              message: error instanceof Error ? error.message : 'Could not verify your identity.',
            });
          }
        });
    };
    const visible = () => {
      if (document.visibilityState !== 'hidden') refresh();
    };
    refresh();
    window.addEventListener('focus', refresh);
    window.addEventListener('online', refresh);
    document.addEventListener('visibilitychange', visible);

    return () => {
      live = false;
      window.removeEventListener('focus', refresh);
      window.removeEventListener('online', refresh);
      document.removeEventListener('visibilitychange', visible);
    };
  }, [provider]);

  const signIn = useCallback(
    async (returnTo?: string, options?: SignInOptions) => {
      const version = ++currentRead.current;
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
        if (version !== currentRead.current) return;
        if (next.status === 'authenticated') retainTaskViewer(next.user.id);
        if (next.status === 'anonymous') retainTaskViewer();
        setState(next);
      } catch (error: unknown) {
        if (version !== currentRead.current) return;
        setState({
          status: 'error',
          message: error instanceof Error ? error.message : 'Sign-in could not be started.',
        });
      }
    },
    [provider],
  );

  const signOut = useCallback(async () => {
    const version = ++currentRead.current;
    try {
      await provider.signOut();
      if (version !== currentRead.current) return;
      retainTaskViewer();
      setState({ status: 'anonymous' });
    } catch (error: unknown) {
      if (version !== currentRead.current) return;
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
