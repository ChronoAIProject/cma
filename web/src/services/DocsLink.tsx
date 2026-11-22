import type { ReactNode } from 'react';
import type { DocsView } from '../routing/docsRoutes';
import { navigateAppPath } from '../routing/navigationGuard';
import { routePath, type AppRoute } from '../routing/routes';
import './DocsLink.css';

export function DocsLink({
  view,
  children,
  className = 'task-help-link',
  onNavigate,
}: {
  readonly view: DocsView;
  readonly children: ReactNode;
  readonly className?: string;
  readonly onNavigate?: (route: AppRoute) => void;
}) {
  const route: AppRoute = { kind: 'docs', view };
  const href = routePath(route);
  return (
    <a
      className={className}
      href={href}
      onClick={(event) => {
        if (event.button || event.ctrlKey || event.metaKey || event.shiftKey || event.altKey)
          return;
        event.preventDefault();
        if (onNavigate) onNavigate(route);
        else navigateAppPath(href);
      }}
    >
      {children}
    </a>
  );
}
