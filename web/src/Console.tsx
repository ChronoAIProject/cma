import { Presence } from './ui/Presence';
import { lazy, Suspense, useCallback, useEffect, useState } from 'react';
import { LandingCharacter } from './landing/LandingCharacter';
import {
  useCmaClient,
  useProfileViewsClient,
  useOperatorClient,
  usePublisherClient,
} from './api/context';
import { canOpenOperations } from './operator/capabilities';
import { AgentSurface } from './agent/AgentSurface';
import { AgentDetails } from './agent/Details';
import { useAgentController } from './agent/controller';
import { sameAgent } from './agent/status';
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
import type { ProfileUse } from './profiles/marketplace/client';

const ProfileViews = lazy(() =>
  import('./profiles/ProfileViews').then((module) => ({ default: module.ProfileViews })),
);
const Operations = lazy(() =>
  import('./operator/Operations').then((module) => ({ default: module.Operations })),
);
const TriggerPublisherPage = lazy(() =>
  import('./trigger-publisher/TriggerPublisherPage').then((module) => ({
    default: module.TriggerPublisherPage,
  })),
);

interface ConsoleProps {
  readonly user: User;
  readonly route: AppRoute;
  readonly onSignOut: () => void;
  readonly onNavigate: (route: AppRoute) => void;
  readonly onCreate?: (action: CreateAction) => void;
  readonly onUseProfile?: (selection: ProfileUse) => void;
  readonly hierarchyRevision?: number;
}

export function Console({
  user,
  route,
  onSignOut,
  onNavigate,
  onCreate,
  onUseProfile,
  hierarchyRevision = 0,
}: ConsoleProps) {
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  const [detailsOpen, setDetailsOpen] = useState(false);
  const [revision, setRevision] = useState(0);
  const client = useCmaClient();
  const profiles = useProfileViewsClient();
  const operator = useOperatorClient();
  const publisher = usePublisherClient();
  const navigation = useNavigation(client);
  const refreshNavigation = navigation.refresh;
  useEffect(() => {
    refreshNavigation();
  }, [hierarchyRevision, refreshNavigation]);
  const refresh = useCallback(() => {
    refreshNavigation();
    setRevision((value) => value + 1);
  }, [refreshNavigation]);
  const agent = useAgentController(client, route.kind === 'agent' ? route.agentId : undefined, {
    summary:
      route.kind === 'agent' && navigation.status === 'ready'
        ? navigation.items
            .flatMap(({ workspaces }) => workspaces.flatMap((workspace) => workspace.agents))
            .find(({ id }) => id === route.agentId)
        : undefined,
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
    user.id,
  );
  const canUse = user.capabilities.includes('use_own_sandboxes');
  const canCreate = !user.visitor && canUse && onCreate !== undefined;
  const canCreateSandbox = canCreate && user.capabilities.includes('create_sandbox');
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
    ) : route.kind === 'profiles' ||
      route.kind === 'profile' ||
      route.kind === 'marketplace' ||
      route.kind === 'marketplace_profile' ? (
      profiles === undefined ? (
        <section className="console__empty">
          <h1>Profiles unavailable</h1>
        </section>
      ) : (
        <Suspense
          fallback={
            <section className="console__empty" role="status">
              Loading profiles
            </section>
          }
        >
          <ProfileViews
            key={JSON.stringify(route)}
            route={route}
            client={profiles}
            onNavigate={onNavigate}
            canManage={!user.visitor && user.capabilities.includes('manage_own_profiles')}
            onUse={
              canCreate &&
              (route.kind !== 'marketplace_profile' ||
                route.profileKind !== 'sandbox' ||
                canCreateSandbox)
                ? onUseProfile
                : undefined
            }
          />
        </Suspense>
      )
    ) : route.kind === 'operations' ? (
      operator !== undefined && canOpenOperations(user.capabilities) ? (
        <Suspense
          fallback={
            <section className="console__empty" role="status">
              Loading Operations
            </section>
          }
        >
          <Operations client={operator} capabilities={user.capabilities} />
        </Suspense>
      ) : (
        <section className="console__empty">
          <h1>Operations unavailable</h1>
        </section>
      )
    ) : route.kind === 'triggers' || route.kind === 'trigger' || route.kind === 'trigger_embed' ? (
      publisher !== undefined && !user.visitor && user.capabilities.includes('publish_triggers') ? (
        <Suspense
          fallback={
            <section className="console__empty" role="status">
              Loading CMA Trigger
            </section>
          }
        >
          <TriggerPublisherPage
            key={route.kind === 'triggers' ? 'new' : route.triggerId}
            client={publisher}
            triggerId={route.kind === 'triggers' ? undefined : route.triggerId}
            view={route.kind === 'trigger_embed' ? 'embed' : 'wizard'}
            onNavigate={onNavigate}
          />
        </Suspense>
      ) : (
        <section className="console__empty">
          <h1>Publishing unavailable</h1>
        </section>
      )
    ) : route.kind === 'not_found' ? (
      <section className="console__empty" aria-labelledby="not-found-title">
        <p className="console__eyebrow">404</p>
        <h1 id="not-found-title">Page not found</h1>
        <a href="/">Return to CMA</a>
      </section>
    ) : (
      <section className="console__empty" aria-labelledby="empty-title">
        <div className="brand-home brand-home--compact">
          <LandingCharacter autoRotate />
        </div>
        <h1 id="empty-title">
          {route.kind === 'home' ? 'Welcome to CMA' : route.kind.replaceAll('_', ' ')}
        </h1>
        {route.kind === 'home' && canCreate ? (
          <div className="console__empty-actions">
            <Button
              variant="primary"
              icon={<IconNewChatOutline16 />}
              onClick={() => onCreate?.({ kind: 'new_agent' })}
            >
              New agent
            </Button>
            {canCreateSandbox ? (
              <Button onClick={() => onCreate?.({ kind: 'new_sandbox' })}>New sandbox</Button>
            ) : null}
          </div>
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
            historyLoading={ready?.historyLoading}
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
      sidebar={({ collapsed, onToggle, onNavigate: onSidebarNavigate }) => (
        <Sidebar
          collapsed={collapsed}
          onToggleCollapsed={onToggle}
          canCreateAgent={canCreate}
          onNewAgent={() => onCreate?.({ kind: 'new_agent' })}
          onHome={() => {
            setDetailsOpen(false);
            onSidebarNavigate();
            onNavigate({ kind: 'home' });
          }}
          navigation={
            navigation.status === 'ready' ? (
              <Tree
                key={user.id}
                viewerId={user.id}
                items={navigation.items.map((item) => ({
                  ...item,
                  workspaces: item.workspaces.map((workspace) => ({
                    ...workspace,
                    agents: workspace.agents.map((summary) =>
                      route.kind === 'agent' &&
                      agent.state.status === 'ready' &&
                      agent.state.detail.id === route.agentId &&
                      sameAgent(agent.state.detail, summary)
                        ? { ...summary, status: agent.state.detail.status }
                        : summary,
                    ),
                  })),
                }))}
                refreshError={navigation.refreshError}
                lifecycleBusy={actions.pending}
                route={route}
                permissions={permissions}
                onNavigate={(next) => {
                  onSidebarNavigate();
                  onNavigate(next);
                }}
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
          {actions.notice}
          <Presence>
            {actions.error === undefined ? null : (
              <p className="console__action-error" role="alert">
                {actions.error}
              </p>
            )}
          </Presence>
          {content}
        </main>
      }
    />
  );
}
