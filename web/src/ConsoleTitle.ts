import type { AppRoute } from './routing/routes';
import { useDocumentTitle } from './ui/useDocumentTitle';

export function useConsoleDocumentTitle(route: AppRoute, hasAccessibleAgent: boolean) {
  useDocumentTitle(
    route.kind === 'home' || route.kind === 'agent_manager'
      ? 'Agent Manager'
      : route.kind === 'not_found'
        ? 'Page not found'
        : undefined,
    route.kind === 'agent' && hasAccessibleAgent ? 'inherit' : 'product',
  );
}
