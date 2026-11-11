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
  readonly outage: Extract<AuthState, { status: 'error' }> | undefined;
  readonly retry: () => void;
  readonly signIn: (returnTo?: string, options?: SignInOptions) => Promise<void>;
  readonly signOut: () => Promise<void>;
}

const AuthContext = createContext<AuthContextValue | null>(null);

const SILENT_SIGN_IN_TIMEOUT_MS = 10000;

interface AuthProviderProps {
  readonly provider: AuthProvider;
  readonly children: ReactNode;
}

export function AuthContextProvider({ provider, children }: AuthProviderProps) {
  const [state, setState] = useState<AuthState>({ status: 'checking' });
  const currentRead = useRef(0);
  const [outage, setOutage] = useState<Extract<AuthState, { status: 'error' }>>();
  const refreshRef = useRef<() => void>(() => {});
  const retry = useCallback(() => refreshRef.current(), []);

  useEffect(() => {
    // Guards against a resolved promise landing after unmount, which React 19
    // StrictMode makes routine by running effects twice in development.
    let live = true;
    let inFlight = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let failures = 0;
    let last: AuthState = { status: 'checking' };
    let pendingNavigation = false;
    let leaving: ReturnType<typeof setTimeout> | undefined;

    const refresh = (mayNavigate = false) => {
      if (!live || inFlight || leaving !== undefined) return;
      clearTimeout(timer);
      inFlight = true;
      const version = ++currentRead.current;
      void provider
        .current()
        .then((next) => {
          if (live && version === currentRead.current) {
            if (next.status !== 'error') last = next;
            let leavingSilently = false;
            if (mayNavigate && next.status === 'anonymous') {
              try {
                leavingSilently = provider.signInSilently?.() === true;
              } catch {
                /* Show the signed-out page. */
              }
            }
            if (leavingSilently && next.status === 'anonymous') {
              setOutage(undefined);
              setState({ status: 'checking' });
              leaving = setTimeout(() => {
                leaving = undefined;
                if (!live || version !== currentRead.current) return;
                setState(next);
              }, SILENT_SIGN_IN_TIMEOUT_MS);
              return;
            }
            if (next.status === 'authenticated') retainTaskViewer(next.user.id);
            if (next.status === 'error' && next.reason !== undefined) {
              setOutage(next);
              // Retain mounted views and drafts. This is presentation only;
              // every protected operation still requires server authorization.
              setState((previous) => (previous.status === 'authenticated' ? previous : next));
              const delay = Math.min(3000 * 2 ** Math.min(failures++, 4), 30000);
              timer = setTimeout(
                () => refresh(mayNavigate),
                delay + Math.floor(Math.random() * 501),
              );
            } else {
              failures = 0;
              setOutage(undefined);
              setState(next);
            }
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
        })
        .finally(() => {
          inFlight = false;
          if (live && pendingNavigation) {
            pendingNavigation = false;
            refresh(true);
          }
        });
    };
    const backgroundRefresh = () => refresh();
    refreshRef.current = backgroundRefresh;
    const navigation = () => {
      if (last.status !== 'anonymous') return;
      if (inFlight) {
        pendingNavigation = true;
        return;
      }
      refresh(true);
    };
    const visible = () => {
      if (document.visibilityState !== 'hidden') refresh();
    };
    refresh(true);
    window.addEventListener('popstate', navigation);
    window.addEventListener('cma:navigate', navigation);
    window.addEventListener('focus', backgroundRefresh);
    window.addEventListener('online', backgroundRefresh);
    document.addEventListener('visibilitychange', visible);

    return () => {
      live = false;
      clearTimeout(timer);
      clearTimeout(leaving);
      refreshRef.current = () => {};
      window.removeEventListener('popstate', navigation);
      window.removeEventListener('cma:navigate', navigation);
      window.removeEventListener('focus', backgroundRefresh);
      window.removeEventListener('online', backgroundRefresh);
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
        setOutage(undefined);
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
      setOutage(undefined);
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
    () => ({ state, outage, retry, signIn, signOut }),
    [state, outage, retry, signIn, signOut],
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
