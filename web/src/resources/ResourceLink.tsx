import type { ReactNode } from 'react';
import { routePath, type AppRoute } from '../routing/routes';

export function ResourceLink({
  route,
  onNavigate,
  children,
}: {
  readonly route: AppRoute;
  readonly onNavigate: (route: AppRoute) => void;
  readonly children: ReactNode;
}) {
  return (
    <a
      href={routePath(route)}
      onClick={(event) => {
        if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey)
          return;
        event.preventDefault();
        onNavigate(route);
      }}
    >
      {children}
    </a>
  );
}
