import type { User } from './auth/types';
import type { AppRoute } from './routing/routes';
import type { CreateAction } from './ResourceActions';

export interface ConsoleProps {
  readonly user: User;
  readonly sidebarCollapsed?: boolean;
  readonly onSidebarCollapsedChange?: (collapsed: boolean) => void;
  readonly onServiceNavigate?: (route: AppRoute) => void;
  readonly route: AppRoute;
  readonly onSignOut: () => void;
  readonly onDeleteAccount?: () => void;
  readonly onNavigate: (route: AppRoute) => void;
  readonly onCreate?: (action: CreateAction) => void;
  readonly hierarchyRevision?: number;
}
