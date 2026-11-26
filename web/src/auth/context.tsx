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
import { flushSync } from 'react-dom';
import type { AuthProvider, AuthState, SignInOptions } from './types';
import { parseRoute, routePath } from '../routing/routes';
import { retainDocumentShareToken } from '../documents/shareToken';
import { retainAgentShareForSignIn } from '../resources/agentShareToken';
import { retainTaskViewer } from '../create/taskIdentity';
import { retainAgentDraftOwner } from '../agent/draftIdentity';
import { clearAgentTransport } from '../agent/transport/journal';
import { clearNewChannelDraft } from '../services/channelForm/newChannelDraft';
import { retainPreparationDraftOwner } from '../preparation/preparationDraftIdentity';
import { releaseNavigationGuard } from '../routing/navigationGuard';
import {
  isSignOutNotice,
  openSignOutChannel,
  readSignOutNotice,
  SIGN_OUT_NOTICE_KEY,
} from './signOutNotice';

/**
 * Wires an AuthProvider into React. Components consume `useAuth()` and never
 * reference a concrete provider, so the mock and the real NyxID adapter are
 * interchangeable without touching a single consumer.
 */

interface AuthContextValue {
  readonly state: AuthState;
  readonly outage: Extract<AuthState, { status: 'error' }> | undefined;
  readonly sessionReadSignal: AbortSignal;
  readonly explicitlySignedOut: boolean;
  readonly retry: () => void;
  readonly signIn: (returnTo?: string, options?: SignInOptions) => Promise<void>;
  readonly signOut: () => Promise<void>;
}

const AuthContext = createContext<AuthContextValue | null>(null);

const SILENT_SIGN_IN_TIMEOUT_MS = 10000;
const SESSION_READ_PAUSED = AbortSignal.abort();

const MAX_RETRY_DELAY_MS = 2_147_483_647;

function serverRetryFloor(state: AuthState): number {
  const retry = state.status === 'error' ? state.retry : undefined;
  const delay = retry?.kind === 'http' ? retry.retryAfterMs : undefined;
  return retry?.kind === 'http' &&
    [429, 502, 503, 504].includes(retry.status) &&
    typeof delay === 'number' &&
    Number.isSafeInteger(delay) &&
    delay > 0 &&
    delay <= MAX_RETRY_DELAY_MS
    ? delay
    : 0;
}

function clearSignedOutTab() {
  releaseNavigationGuard();
  retainTaskViewer();
  retainAgentDraftOwner();
  retainPreparationDraftOwner();
  clearNewChannelDraft();
  try {
    clearAgentTransport(sessionStorage);
  } catch {
    // A storage failure must not keep the protected view mounted.
  }
}

function hasNewSignOut(previous: string | null): boolean {
  const notice = readSignOutNotice();
  return notice !== null && notice !== previous;
}

interface AuthProviderProps {
  readonly provider: AuthProvider;
  readonly children: ReactNode;
}

export function AuthContextProvider({ provider, children }: AuthProviderProps) {
  const [state, setState] = useState<AuthState>({ status: 'checking' });
  const [explicitlySignedOut, setExplicitlySignedOut] = useState(false);
  const signedOut = useRef(false);
  const signOutNotice = useRef<string | null>(null);
  const currentRead = useRef(0);
  const sessionReads = useRef<AbortController | undefined>(undefined);
  const [sessionReadSignal, setSessionReadSignal] = useState(SESSION_READ_PAUSED);
  const allowSessionReads = useCallback((allowed: boolean) => {
    if (!allowed) {
      sessionReads.current?.abort();
      sessionReads.current = undefined;
      setSessionReadSignal(SESSION_READ_PAUSED);
    } else if (sessionReads.current === undefined) {
      sessionReads.current = new AbortController();
      setSessionReadSignal(sessionReads.current.signal);
    }
  }, []);
  const [outage, setOutage] = useState<Extract<AuthState, { status: 'error' }>>();
  const refreshRef = useRef<() => void>(() => {});
  const suspendRecoveryRef = useRef<() => (next?: AuthState) => void>(() => () => {});
  const retry = useCallback(() => refreshRef.current(), []);

  useEffect(() => {
    // Guards against a resolved promise landing after unmount, which React 19
    // StrictMode makes routine by running effects twice in development.
    let live = true;
    let inFlight: number | undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let failures = 0;
    let retryNotBefore = 0;
    let suspended = false;
    let last: AuthState = { status: 'checking' };
    let pendingNavigation = false;
    let pendingRefresh = false;
    let leaving: ReturnType<typeof setTimeout> | undefined;
    signOutNotice.current = readSignOutNotice();
    const seenNotices = new Set([signOutNotice.current]);
    const channel = openSignOutChannel();

    suspendRecoveryRef.current = () => {
      suspended = true;
      clearTimeout(timer);
      clearTimeout(leaving);
      leaving = undefined;
      retryNotBefore = 0;
      failures = 0;
      pendingNavigation = false;
      pendingRefresh = false;
      last = { status: 'checking' };
      return (next) => {
        if (next !== undefined) {
          retryNotBefore = performance.now() + serverRetryFloor(next);
          if (next.status !== 'error') last = next;
        }
        suspended = false;
      };
    };
    const receiveSignOut = (notice: string, synchronous: boolean) => {
      if (seenNotices.has(notice)) return;
      seenNotices.add(notice);
      const invalidate = () => {
        currentRead.current++;
        suspendRecoveryRef.current();
        signedOut.current = true;
        setExplicitlySignedOut(true);
        setOutage(undefined);
        setState({ status: 'anonymous' });
        allowSessionReads(false);
        clearSignedOutTab();
      };
      if (synchronous) flushSync(invalidate);
      else invalidate();
    };
    const observeSignOut = (synchronous = false) => {
      const notice = readSignOutNotice();
      if (notice === null) return;
      signOutNotice.current = notice;
      receiveSignOut(notice, synchronous);
    };
    const storage = (event: StorageEvent) => {
      if (event.key === SIGN_OUT_NOTICE_KEY) observeSignOut(true);
    };
    const broadcast = ({ data }: MessageEvent<unknown>) => {
      observeSignOut(true);
      if (isSignOutNotice(data)) receiveSignOut(data, true);
    };
    const refresh = (mayNavigate = false) => {
      if (!live) return;
      observeSignOut();
      if (signedOut.current || suspended || leaving !== undefined) return;
      if (inFlight !== undefined) {
        pendingRefresh = true;
        return;
      }
      clearTimeout(timer);
      const remaining = retryNotBefore - performance.now();
      if (remaining > 0) {
        timer = setTimeout(
          () => refresh(mayNavigate),
          Math.min(MAX_RETRY_DELAY_MS, Math.ceil(remaining)),
        );
        return;
      }
      allowSessionReads(false);
      const version = ++currentRead.current;
      inFlight = version;
      let canFollowUp = false;
      void provider
        .current()
        .then((next) => {
          if (live) observeSignOut();
          if (live && version === currentRead.current) {
            retryNotBefore = performance.now() + serverRetryFloor(next);
            canFollowUp = next.status === 'authenticated' || next.status === 'anonymous';
            if (next.status === 'authenticated') {
              retainAgentDraftOwner(next.user.id);
              retainPreparationDraftOwner(next.user.id);
            } else if (next.status === 'anonymous') {
              releaseNavigationGuard();
              // A lapsed session hides the composer until the owner is verified again.
              // Explicit sign-out and a different verified owner clear the saved Agent draft.
              retainPreparationDraftOwner();
            }
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
            allowSessionReads(next.status === 'authenticated');
            if (next.status === 'error' && next.reason !== undefined) {
              setOutage(next);
              // Retain mounted views and drafts. This is presentation only;
              // every protected operation still requires server authorization.
              setState(last.status === 'authenticated' ? last : next);
              const delay = Math.min(3000 * 2 ** Math.min(failures++, 4), 30000);
              timer = setTimeout(
                () => refresh(mayNavigate),
                Math.min(
                  MAX_RETRY_DELAY_MS,
                  Math.max(
                    delay + Math.floor(Math.random() * 501),
                    Math.ceil(retryNotBefore - performance.now()),
                  ),
                ),
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
          if (inFlight !== version) return;
          inFlight = undefined;
          const followUp = pendingNavigation || (pendingRefresh && canFollowUp);
          const navigate = pendingNavigation;
          pendingNavigation = false;
          pendingRefresh = false;
          if (live && version === currentRead.current && followUp) refresh(navigate);
        });
    };
    const backgroundRefresh = () => refresh();
    refreshRef.current = backgroundRefresh;
    const navigation = () => {
      observeSignOut(true);
      if (last.status !== 'anonymous') return;
      if (inFlight !== undefined) {
        pendingNavigation = true;
        return;
      }
      refresh(true);
    };
    const visible = () => {
      if (document.visibilityState !== 'hidden') {
        observeSignOut(true);
        refresh();
      }
    };
    const hideCachedPage = (event: PageTransitionEvent) => {
      if (!event.persisted || signedOut.current) return;
      flushSync(() => {
        if (!suspended) currentRead.current++;
        inFlight = undefined;
        clearTimeout(timer);
        clearTimeout(leaving);
        leaving = undefined;
        pendingRefresh = false;
        pendingNavigation = false;
        last = { status: 'checking' };
        allowSessionReads(false);
        setOutage(undefined);
        setState(last);
      });
    };
    const restore = (event: PageTransitionEvent) => {
      observeSignOut(true);
      if (event.persisted && !signedOut.current) {
        hideCachedPage(event);
        refresh();
      }
    };
    channel?.addEventListener('message', broadcast);
    window.addEventListener('storage', storage);
    window.addEventListener('pagehide', hideCachedPage);
    window.addEventListener('pageshow', restore);
    refresh(true);
    window.addEventListener('popstate', navigation);
    window.addEventListener('cma:navigate', navigation);
    window.addEventListener('focus', backgroundRefresh);
    window.addEventListener('online', backgroundRefresh);
    document.addEventListener('visibilitychange', visible);

    return () => {
      live = false;
      currentRead.current += 1;
      sessionReads.current?.abort();
      sessionReads.current = undefined;
      clearTimeout(timer);
      clearTimeout(leaving);
      refreshRef.current = () => {};
      suspendRecoveryRef.current = () => () => {};
      channel?.close();
      window.removeEventListener('storage', storage);
      window.removeEventListener('pagehide', hideCachedPage);
      window.removeEventListener('pageshow', restore);
      window.removeEventListener('popstate', navigation);
      window.removeEventListener('cma:navigate', navigation);
      window.removeEventListener('focus', backgroundRefresh);
      window.removeEventListener('online', backgroundRefresh);
      document.removeEventListener('visibilitychange', visible);
    };
  }, [provider, allowSessionReads]);

  const signIn = useCallback(
    async (returnTo?: string, options?: SignInOptions) => {
      const resume = suspendRecoveryRef.current();
      const version = ++currentRead.current;
      allowSessionReads(false);
      let next: AuthState | undefined;
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
        if (version !== currentRead.current || hasNewSignOut(signOutNotice.current)) return;
        next = await provider.current();
        if (version !== currentRead.current || hasNewSignOut(signOutNotice.current)) return;
        if (next.status === 'authenticated') {
          signedOut.current = false;
          setExplicitlySignedOut(false);
          retainTaskViewer(next.user.id);
          retainAgentDraftOwner(next.user.id);
          retainPreparationDraftOwner(next.user.id);
        } else if (next.status === 'anonymous') {
          retainPreparationDraftOwner();
        }
        allowSessionReads(next.status === 'authenticated');
        setOutage(undefined);
        setState(next);
      } catch (error: unknown) {
        if (version !== currentRead.current) return;
        setState({
          status: 'error',
          message: error instanceof Error ? error.message : 'Sign-in could not be started.',
        });
      } finally {
        if (version === currentRead.current) resume(next);
      }
    },
    [provider, allowSessionReads],
  );

  const signOut = useCallback(async () => {
    const resume = suspendRecoveryRef.current();
    const version = ++currentRead.current;
    allowSessionReads(false);
    let next: AuthState | undefined;
    try {
      clearSignedOutTab();
      setState({ status: 'checking' });
      await provider.signOut();
      if (version !== currentRead.current) return;
      signOutNotice.current = readSignOutNotice();
      signedOut.current = true;
      setExplicitlySignedOut(true);
      setOutage(undefined);
      next = { status: 'anonymous' };
      setState(next);
    } catch (error: unknown) {
      if (version !== currentRead.current) return;
      setState({
        status: 'error',
        message: error instanceof Error ? error.message : 'Sign-out did not complete.',
      });
    } finally {
      if (version === currentRead.current) resume(next);
    }
  }, [provider, allowSessionReads]);

  const value = useMemo<AuthContextValue>(
    () => ({ state, outage, sessionReadSignal, explicitlySignedOut, retry, signIn, signOut }),
    [state, outage, sessionReadSignal, explicitlySignedOut, retry, signIn, signOut],
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

export function useIdentityUncertain(): boolean {
  const value = useContext(AuthContext);
  return value !== null && (value.outage !== undefined || value.state.status !== 'authenticated');
}

/** Account identity for optional owner controls; absent outside an authenticated shell. */
export function useAuthenticatedSubject(): string | undefined {
  const value = useContext(AuthContext);
  return value?.state.status === 'authenticated' ? value.state.user.id : undefined;
}

/** Access to background reads backed by the current same-origin session. */
export function useSessionReadSignal(): AbortSignal | undefined {
  return useContext(AuthContext)?.sessionReadSignal;
}
