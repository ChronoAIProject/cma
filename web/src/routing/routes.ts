import { useCallback, useEffect, useState } from 'react';

export type AppRoute =
  { readonly kind: 'home' } | { readonly kind: 'not_found'; readonly pathname: string };

export function parseRoute(pathname: string): AppRoute {
  return pathname === '/' || pathname === '' ? { kind: 'home' } : { kind: 'not_found', pathname };
}

export function routePath(route: AppRoute): string {
  return route.kind === 'home' ? '/' : route.pathname;
}

export interface AppRouter {
  readonly route: AppRoute;
  readonly navigate: (route: AppRoute, options?: { readonly replace?: boolean }) => void;
}

export function useAppRouter(): AppRouter {
  const [route, setRoute] = useState<AppRoute>(() => parseRoute(window.location.pathname));

  useEffect(() => {
    const onPopState = () => {
      setRoute(parseRoute(window.location.pathname));
    };
    window.addEventListener('popstate', onPopState);
    return () => {
      window.removeEventListener('popstate', onPopState);
    };
  }, []);

  const navigate = useCallback<AppRouter['navigate']>((next, options) => {
    const path = routePath(next);
    if (options?.replace === true) {
      window.history.replaceState(null, '', path);
    } else {
      window.history.pushState(null, '', path);
    }
    setRoute(next);
  }, []);

  return { route, navigate };
}
