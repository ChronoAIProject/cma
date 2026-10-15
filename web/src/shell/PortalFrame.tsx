import type { ReactNode } from 'react';
import { useAuth } from '../auth/context';
import { useCmaClient } from '../api/context';
import { canManageBots } from '../BotManagement';
import { ProfileMenu } from '../profile/ProfileMenu';
import type { AppRoute } from '../routing/routes';
import { AppFrame } from './AppFrame';
import { Sidebar } from './Sidebar';

/** All portals share the same frame, brand, navigation switch and account controls. */
export function PortalFrame({
  route,
  onNavigate,
  navigation,
  children,
  landing = false,
  sidebarCollapsed,
  onSidebarCollapsedChange,
}: {
  readonly route: AppRoute;
  readonly onNavigate: (route: AppRoute) => void;
  readonly navigation?: ReactNode;
  readonly children: ReactNode;
  readonly landing?: boolean;
  readonly sidebarCollapsed: boolean;
  readonly onSidebarCollapsedChange: (collapsed: boolean) => void;
}) {
  const { state, signOut, signIn } = useAuth();
  const client = useCmaClient();
  return (
    <AppFrame
      detailsOpen={false}
      sidebarCollapsed={sidebarCollapsed}
      onSidebarCollapsedChange={onSidebarCollapsedChange}
      sidebar={({ collapsed, onToggle, onNavigate: closeNavigation }) => (
        <Sidebar
          collapsed={collapsed}
          onToggleCollapsed={onToggle}
          route={route}
          onNavigate={(next) => {
            closeNavigation();
            onNavigate(next);
          }}
          navigation={navigation}
          account={
            state.status === 'authenticated' ? (
              <ProfileMenu
                user={state.user}
                bots={canManageBots(state.user, client)}
                collapsed={collapsed}
                onNavigate={(next) => {
                  closeNavigation();
                  onNavigate(next);
                }}
                onSignOut={() => void signOut()}
              />
            ) : (
              <button
                type="button"
                className="sidebar__new"
                title="Sign in"
                aria-label="Sign in"
                onClick={() => void signIn('/agent-manager')}
              >
                {collapsed ? '→' : 'Sign in'}
              </button>
            )
          }
        />
      )}
      center={
        <div className={landing || route.kind === 'home' ? 'portal-home' : 'service-page'}>
          {children}
        </div>
      }
    />
  );
}
