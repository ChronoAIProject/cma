import { useCallback, useEffect, useState } from 'react';
import type { MouseEvent as ReactMouseEvent } from 'react';

/** The complete browser-route vocabulary defined by Web_Style.md section 13b. */
export type TriggerAuthError = 'expired' | 'cancelled' | 'failed';

export type AppRoute =
  | { readonly kind: 'home' }
  | {
      readonly kind: 'trigger';
      readonly publicId: string;
      readonly authError?: TriggerAuthError;
    }
  | { readonly kind: 'trigger_recovery'; readonly launchId: string }
  | { readonly kind: 'launch'; readonly launchId: string }
  | { readonly kind: 'contribute'; readonly launchId: string }
  | { readonly kind: 'session'; readonly sessionId: string }
  | { readonly kind: 'not_found'; readonly pathname: string };

function segment(value: string): string | undefined {
  try {
    const decoded = decodeURIComponent(value);
    return decoded !== '' && decoded !== '.' && decoded !== '..' && !decoded.includes('/')
      ? decoded
      : undefined;
  } catch {
    return undefined;
  }
}

function triggerAuthError(search: string): TriggerAuthError | undefined {
  const values = new URLSearchParams(search).getAll('auth_error');
  if (values.length !== 1) return undefined;
  const [value] = values;
  return value === 'expired' || value === 'cancelled' || value === 'failed' ? value : undefined;
}

/** Parse one location; query data is retained only as a closed, display-only error hint. */
export function parseRoute(pathname: string, search = ''): AppRoute {
  if (pathname === '/' || pathname === '') {
    return { kind: 'home' };
  }
  const parts = pathname.split('/');
  if (parts[0] !== '') {
    return { kind: 'not_found', pathname };
  }

  const identifier = parts[2] === undefined ? undefined : segment(parts[2]);
  if (parts.length === 3 && parts[1] === 'triggers' && identifier !== undefined) {
    const authError = triggerAuthError(search);
    return authError === undefined
      ? { kind: 'trigger', publicId: identifier }
      : { kind: 'trigger', publicId: identifier, authError };
  }
  if (parts.length === 3 && parts[1] === 'launches' && identifier !== undefined) {
    return { kind: 'launch', launchId: identifier };
  }
  if (
    parts.length === 4 &&
    parts[1] === 'launches' &&
    identifier !== undefined &&
    parts[3] === 'recover'
  ) {
    return { kind: 'trigger_recovery', launchId: identifier };
  }
  if (
    parts.length === 4 &&
    parts[1] === 'launches' &&
    identifier !== undefined &&
    parts[3] === 'contribute'
  ) {
    return { kind: 'contribute', launchId: identifier };
  }
  if (parts.length === 3 && parts[1] === 'sessions' && identifier !== undefined) {
    return { kind: 'session', sessionId: identifier };
  }
  return { kind: 'not_found', pathname };
}

export function routePath(route: Exclude<AppRoute, { readonly kind: 'not_found' }>): string {
  switch (route.kind) {
    case 'home':
      return '/';
    case 'trigger':
      return `/triggers/${encodeURIComponent(route.publicId)}`;
    case 'trigger_recovery':
      return `/launches/${encodeURIComponent(route.launchId)}/recover`;
    case 'launch':
      return `/launches/${encodeURIComponent(route.launchId)}`;
    case 'contribute':
      return `/launches/${encodeURIComponent(route.launchId)}/contribute`;
    case 'session':
      return `/sessions/${encodeURIComponent(route.sessionId)}`;
    default: {
      const exhaustive: never = route;
      return exhaustive;
    }
  }
}

export type NavigableRoute = Exclude<AppRoute, { readonly kind: 'not_found' }>;

export interface AppRouter {
  readonly route: AppRoute;
  readonly navigate: (route: NavigableRoute, options?: { readonly replace?: boolean }) => void;
}

/** A deliberately small History API router: navigation changes views and nothing else. */
export function useAppRouter(): AppRouter {
  const [route, setRoute] = useState<AppRoute>(() =>
    parseRoute(window.location.pathname, window.location.search),
  );

  useEffect(() => {
    const onPopState = () => {
      setRoute(parseRoute(window.location.pathname, window.location.search));
    };
    window.addEventListener('popstate', onPopState);
    return () => {
      window.removeEventListener('popstate', onPopState);
    };
  }, []);

  const navigate = useCallback<AppRouter['navigate']>((next, options) => {
    const path = routePath(next);
    if (path === window.location.pathname && window.location.search === '') {
      setRoute(next);
      return;
    }
    if (options?.replace === true) {
      window.history.replaceState(null, '', path);
    } else {
      window.history.pushState(null, '', path);
    }
    setRoute(next);
  }, []);

  return { route, navigate };
}

/** Preserve ordinary browser link gestures while handling same-origin route clicks. */
export function routeClick(
  event: ReactMouseEvent<HTMLAnchorElement>,
  route: NavigableRoute,
  navigate: AppRouter['navigate'],
): void {
  if (
    event.button !== 0 ||
    event.metaKey ||
    event.ctrlKey ||
    event.shiftKey ||
    event.altKey ||
    event.currentTarget.target === '_blank'
  ) {
    return;
  }
  event.preventDefault();
  navigate(route);
}
