import { Presence } from './ui/Presence';
import { Operations, TriggerPublisherPage } from './ConsolePages';
import { BotManagement, isBotRoute, canManageBots } from './BotManagement';
import { Suspense, useCallback, useEffect, useMemo, useState } from 'react';
import { LandingCharacter } from './landing/LandingCharacter';
import { useCmaClient, useOperatorClient, usePublisherClient } from './api/context';
import { canOpenOperations } from './operator/capabilities';
import { AgentSurface } from './agent/AgentSurface';
import { useAgentDraft } from './agent/draft';
import { AgentDetails } from './agent/Details';
import { useAgentController } from './agent/controller';
import { sameAgent } from './agent/status';
import type { User } from './auth/types';
import { ProfileMenu } from './profile/ProfileMenu';
import { LegacyProfileNotice } from './profile/LegacyProfileNotice';
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
import { useAgentAccess } from './resources/agentAccess';
import { SharedAgent } from './resources/SharedAgent';
import { SharedAgentDetails } from './resources/SharedAgentDetails';
import { AgentInventory } from './resources/AgentInventory';
import { ArtifactPublicationScope } from './resources/artifactRecovery';
import { SandboxAccessPage } from './resources/SandboxAccessPage';
import { WorkspaceContents } from './resources/WorkspaceContents';
import { scopeValue } from './resources/scope';
import { ScopedNavigation } from './resources/ScopedNavigation';
import { NavigationScopePicker, useNavigationScope } from './resources/NavigationScope';
import { useConsoleScope } from './resources/useConsoleScope';

interface ConsoleProps {
  readonly user: User;
  readonly route: AppRoute;
  readonly onSignOut: () => void;
  readonly onNavigate: (route: AppRoute) => void;
  readonly onCreate?: (action: CreateAction) => void;
  readonly hierarchyRevision?: number;
}

export function Console(props: ConsoleProps) {
  return <ConsoleContent key={props.user.id} {...props} />;
}

function ConsoleContent({
  user,
  route,
  onSignOut,
  onNavigate,
  onCreate,
  hierarchyRevision = 0,
}: ConsoleProps) {
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  const [detailsOpen, setDetailsOpen] = useState(false);
  const [artifactPublicationScope] = useState(() => new ArtifactPublicationScope());
  const [revision, setRevision] = useState(0);
  const client = useCmaClient();
  const scopeClient = user.visitor ? undefined : client.resourceViews;
  const {
    value: navigationScope,
    select: setNavigationScope,
    ready: scopeReady,
  } = useConsoleScope(scopeClient, user.id, route);
  const scopeChoices = useNavigationScope(scopeClient, navigationScope);
  const draft = useAgentDraft(client, user.id, route.kind === 'agent' ? route.agentId : undefined);
  const sandboxCommandScope = useMemo(() => ({ client, viewerId: user.id }), [client, user.id]);
  const agentAccess = useAgentAccess(
    client.resourceViews,
    route.kind === 'agent' ? route.agentId : undefined,
    user.id,
    client,
  );
  const accessUncertain = agentAccess.personalReadOnly;
  const shared =
    agentAccess.state.status === 'ready' && agentAccess.state.value.detail.access === 'shared'
      ? agentAccess.state.value.detail
      : undefined;
  const personalAgentId =
    route.kind === 'agent' && (client.resourceViews === undefined || agentAccess.personalReady)
      ? route.agentId
      : undefined;
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
  const agent = useAgentController(client, personalAgentId, {
    suspended: accessUncertain,
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
    onNavigate,
    !accessUncertain,
  );
  const canUse = !accessUncertain && user.capabilities.includes('use_own_sandboxes');
  const canCreate = !user.visitor && canUse && onCreate !== undefined;
  const canCreateInScope = canCreate && navigationScope === '' && scopeReady;
  const permissions: TreePermissions = {
    createAgent: canCreate,
    // Workspace creation is an inline Agent Manager operation. The legacy
    // profile picker remains available only to compatibility handoffs.
    createWorkspace: canCreate,
    pauseSandbox: canUse,
    removeWorkspace: canUse,
    stopAgent: canUse,
    deleteAgent: canUse,
    exportHistory: canUse,
    deleteSandbox: !accessUncertain && user.capabilities.includes('delete_own_sandbox'),
  };
  const ready = agent.state.status === 'ready' ? agent.state : undefined;
  const content = isBotRoute(route) ? (
    <BotManagement user={user} route={route} onNavigate={onNavigate} />
  ) : route.kind === 'inventory' ? (
    client.resourceViews === undefined ? (
      <section className="console__empty">
        <h1>Agent inventory unavailable</h1>
      </section>
    ) : (
      <AgentInventory
        client={client.resourceViews}
        organization={route.organization}
        shared={route.shared}
        onNavigate={(next) => {
          if (next.kind === 'inventory')
            setNavigationScope(scopeValue(next.organization, next.shared));
          onNavigate(next);
        }}
      />
    )
  ) : route.kind === 'agent' && shared !== undefined && client.resourceViews !== undefined ? (
    <SharedAgent
      key={shared.id}
      client={client.resourceViews}
      web={client}
      detail={shared}
      onAccessChanged={agentAccess.reload}
      detailsOpen={detailsOpen}
      onDetailsOpenChange={setDetailsOpen}
    />
  ) : route.kind === 'agent' &&
    client.resourceViews !== undefined &&
    personalAgentId === undefined ? (
    <section
      className="console__empty"
      role={agentAccess.state.status === 'error' ? 'alert' : 'status'}
    >
      <h1>{agentAccess.state.status === 'error' ? 'Agent unavailable' : 'Opening Agent'}</h1>
      {agentAccess.state.status === 'error' ? (
        <>
          <p>{agentAccess.state.message}</p>
          <Button onClick={agentAccess.reload}>Check access again</Button>
        </>
      ) : null}
    </section>
  ) : route.kind === 'agent' ? (
    <AgentSurface
      key={route.agentId}
      lifecycleClient={client}
      draft={draft}
      agentId={route.agentId}
      viewerId={user.id}
      controller={agent}
      accessUncertain={accessUncertain}
      detailsOpen={detailsOpen}
      onDetailsOpenChange={setDetailsOpen}
      onNavigate={onNavigate}
    />
  ) : route.kind === 'sandbox' ? (
    <SandboxAccessPage
      key={route.sandboxId}
      client={client.resourceViews}
      id={route.sandboxId}
      commandScope={sandboxCommandScope}
      onNavigate={onNavigate}
      personal={
        <SandboxPage
          key={route.sandboxId}
          client={client}
          sandboxId={route.sandboxId}
          viewerId={user.id}
          revision={revision}
          permissions={permissions}
          busy={actions.pending || actions.deletingSandboxIds.has(route.sandboxId)}
          onAction={actions.select}
          onNavigate={onNavigate}
        />
      }
    />
  ) : route.kind === 'workspace' ? (
    client.resourceViews === undefined ? (
      <section className="console__empty">
        <h1>Workspace unavailable</h1>
      </section>
    ) : (
      <WorkspaceContents
        key={route.workspaceId}
        client={client.resourceViews}
        id={route.workspaceId}
        onNavigate={onNavigate}
      />
    )
  ) : route.kind === 'event_worker_profiles' ||
    route.kind === 'event_worker_profile' ||
    route.kind === 'profiles' ||
    route.kind === 'profile' ? (
    <LegacyProfileNotice />
  ) : route.kind === 'marketplace' || route.kind === 'marketplace_profile' ? (
    <LegacyProfileNotice />
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
      {route.kind === 'home' && canCreateInScope ? (
        <div className="console__empty-actions">
          <Button
            variant="primary"
            icon={<IconNewChatOutline16 />}
            onClick={() => onCreate?.({ kind: 'new_agent' })}
          >
            New agent
          </Button>
        </div>
      ) : null}
    </section>
  );

  return (
    <AppFrame
      detailsOpen={
        !accessUncertain && (personalAgentId !== undefined || shared !== undefined) && detailsOpen
      }
      compactDetailsOverlay
      details={
        route.kind === 'agent' && personalAgentId !== undefined && !accessUncertain ? (
          <AgentDetails
            key={route.agentId}
            open={detailsOpen}
            onClose={() => setDetailsOpen(false)}
            detail={ready?.detail}
            historyLoading={ready?.historyLoading}
            pending={agent.pending}
            onSandboxProgress={
              ready === undefined
                ? undefined
                : () => onNavigate({ kind: 'sandbox', sandboxId: ready.detail.sandbox.id })
            }
          />
        ) : shared !== undefined ? (
          <SharedAgentDetails
            key={shared.id}
            agent={shared.id}
            open={detailsOpen}
            onClose={() => setDetailsOpen(false)}
            onAccessChanged={agentAccess.reload}
            publicationScope={artifactPublicationScope}
          />
        ) : undefined
      }
      overlay={accessUncertain ? undefined : actions.overlay}
      sidebarCollapsed={sidebarCollapsed}
      onSidebarCollapsedChange={setSidebarCollapsed}
      sidebar={({ collapsed, onToggle, onNavigate: onSidebarNavigate }) => (
        <Sidebar
          collapsed={collapsed}
          onToggleCollapsed={onToggle}
          canCreateAgent={canCreateInScope}
          onNewAgent={() => onCreate?.({ kind: 'new_agent' })}
          onHome={() => {
            setDetailsOpen(false);
            onSidebarNavigate();
            onNavigate({ kind: 'home' });
          }}
          scope={
            scopeClient === undefined ? undefined : (
              <NavigationScopePicker
                choices={scopeChoices}
                value={navigationScope}
                onChange={setNavigationScope}
              />
            )
          }
          navigation={
            <ScopedNavigation
              route={route}
              client={user.visitor ? undefined : client.resourceViews}
              scope={navigationScope}
              choices={scopeChoices}
              onNavigate={(next) => {
                onSidebarNavigate();
                onNavigate(next);
              }}
              personal={
                navigation.status === 'ready' ? (
                  <Tree
                    resourceClient={user.visitor ? undefined : client.resourceViews}
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
                    deletingSandboxIds={actions.deletingSandboxIds}
                    onViewDeletion={actions.viewDeletion}
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
            />
          }
          account={
            <ProfileMenu
              user={user}
              bots={canManageBots(user, client)}
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
