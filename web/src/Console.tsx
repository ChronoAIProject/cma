import { useCallback, useState } from 'react';
import { useCmaClient } from './api/context';
import { AgentSurface } from './agent/AgentSurface';
import { AgentDetails } from './agent/Details';
import { useAgentController } from './agent/controller';
import type { User } from './auth/types';
import { ProfileMenu } from './profile/ProfileMenu';
import type { AppRoute } from './routing/routes';
import { AppFrame } from './shell/AppFrame';
import { Sidebar } from './shell/Sidebar';
import { Tree, type TreePermissions } from './shell/Tree';
import { useNavigation } from './shell/navigation';
import { SandboxPage } from './sandbox/SandboxPage';
import { useResourceActions, type CreateAction } from './ResourceActions';
import { Button } from './ui/Button';
import { IconNewChatOutline16 } from './ui/icons';
import './Console.css';

interface ConsoleProps {
  readonly user: User;
  readonly route: AppRoute;
  readonly onSignOut: () => void;
  readonly onNavigate: (route: AppRoute) => void;
  readonly onCreate?: (action: CreateAction) => void;
}

export function Console({ user, route, onSignOut, onNavigate, onCreate }: ConsoleProps) {
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  const [detailsOpen, setDetailsOpen] = useState(false);
  const [revision, setRevision] = useState(0);
  const client = useCmaClient();
  const navigation = useNavigation(client);
  const refreshNavigation = navigation.refresh;
  const refresh = useCallback(() => {
    refreshNavigation();
    setRevision((value) => value + 1);
  }, [refreshNavigation]);
  const agent = useAgentController(client, route.kind === 'agent' ? route.agentId : undefined, {
    onHierarchyChange: refresh,
    onDeleted: () => onNavigate({ kind: 'home' }),
  });
  const actions = useResourceActions(
    client,
    () => {
      refresh();
      agent.reload();
    },
    onCreate,
  );
  const canUse = user.capabilities.includes('use_own_sandboxes');
  const canCreate = !user.visitor && canUse && onCreate !== undefined;
  const permissions: TreePermissions = {
    createAgent: canCreate,
    createWorkspace: canCreate,
    pauseSandbox: canUse,
    removeWorkspace: canUse,
    stopAgent: canUse,
    deleteAgent: canUse,
    deleteSandbox: user.capabilities.includes('delete_own_sandbox'),
  };
  const ready = agent.state.status === 'ready' ? agent.state : undefined;
  const content =
    route.kind === 'agent' ? (
      <AgentSurface
        key={route.agentId}
        agentId={route.agentId}
        controller={agent}
        detailsOpen={detailsOpen}
        onDetailsOpenChange={setDetailsOpen}
        onNavigate={onNavigate}
      />
    ) : route.kind === 'sandbox' ? (
      <SandboxPage
        key={route.sandboxId}
        client={client}
        sandboxId={route.sandboxId}
        revision={revision}
        permissions={permissions}
        busy={actions.pending}
        onAction={actions.select}
        onNavigate={onNavigate}
      />
    ) : route.kind === 'not_found' ? (
      <section className="console__empty" aria-labelledby="not-found-title">
        <p className="console__eyebrow">404</p>
        <h1 id="not-found-title">Page not found</h1>
        <a href="/">Return to CMA</a>
      </section>
    ) : (
      <section className="console__empty" aria-labelledby="empty-title">
        <p className="console__eyebrow">CMA</p>
        <h1 id="empty-title">
          {route.kind === 'home'
            ? user.visitor
              ? 'Your agents'
              : 'Create your first agent'
            : route.kind.replaceAll('_', ' ')}
        </h1>
        {route.kind === 'home' && canCreate ? (
          <Button
            variant="primary"
            icon={<IconNewChatOutline16 />}
            onClick={() => onCreate?.({ kind: 'new_agent' })}
          >
            New agent
          </Button>
        ) : null}
      </section>
    );

  return (
    <AppFrame
      detailsOpen={route.kind === 'agent' && detailsOpen}
      compactDetailsOverlay
      details={
        route.kind === 'agent' ? (
          <AgentDetails
            key={route.agentId}
            open={detailsOpen}
            onClose={() => setDetailsOpen(false)}
            detail={ready?.detail}
            workspace={ready?.workspace}
            pending={agent.pending}
            actionError={agent.actionError}
            onNavigate={onNavigate}
            onStop={agent.stop}
            onDelete={agent.delete}
          />
        ) : undefined
      }
      overlay={actions.overlay}
      sidebarCollapsed={sidebarCollapsed}
      onSidebarCollapsedChange={setSidebarCollapsed}
      sidebar={({ collapsed, onToggle }) => (
        <Sidebar
          collapsed={collapsed}
          onToggleCollapsed={onToggle}
          canCreateAgent={canCreate}
          onNewAgent={() => onCreate?.({ kind: 'new_agent' })}
          navigation={
            navigation.status === 'ready' ? (
              <Tree
                key={user.id}
                viewerId={user.id}
                items={navigation.items}
                route={route}
                permissions={permissions}
                onNavigate={onNavigate}
                onAction={actions.select}
              />
            ) : navigation.status === 'loading' ? (
              <p className="console__navigation-status" role="status">
                Loading Sandboxes
              </p>
            ) : (
              <div className="console__navigation-status" role="alert">
                <p>{navigation.message}</p>
                <Button onClick={navigation.refresh}>Try again</Button>
              </div>
            )
          }
          account={
            <ProfileMenu
              user={user}
              collapsed={collapsed}
              onSignOut={onSignOut}
              onNavigate={onNavigate}
            />
          }
        />
      )}
      center={
        <main className="console__surface">
          {actions.error === undefined ? null : (
            <p className="console__action-error" role="alert">
              {actions.error}
            </p>
          )}
          {content}
        </main>
      }
    />
  );
}
