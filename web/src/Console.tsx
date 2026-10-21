import { Presence } from './ui/Presence';
import { SkeletonRows } from './ui/Skeleton';
import { Operations, TriggerPublisherPage } from './ConsolePages';
import { BotManagement, isBotRoute } from './BotManagement';
import { lazy, Suspense, useCallback, useEffect, useMemo, useState } from 'react';
import { LandingCharacter } from './landing/LandingCharacter';
import { useCmaClient, useOperatorClient, usePublisherClient } from './api/context';
import { canOpenOperations } from './operator/capabilities';
import { AgentSurface } from './agent/AgentSurface';
import { AgentViewPositions } from './agent/viewPosition';
import { useAgentDraft } from './agent/draft';
import { AgentDetails } from './agent/Details';
import { useAgentController } from './agent/controller';
import type { User } from './auth/types';
import { ProfileMenu } from './profile/ProfileMenu';
import { LegacyProfileNotice } from './profile/LegacyProfileNotice';
import type { AppRoute } from './routing/routes';
import { routePath } from './routing/routes';
import { useViewArrival } from './ui/arrivalMotion';
import { AppFrame } from './shell/AppFrame';
import { Sidebar } from './shell/Sidebar';
import type { TreePermissions } from './shell/Tree';
import { ManagerNavigation } from './shell/ManagerNavigation';
import { useNavigation } from './shell/navigation';
import { SandboxPage } from './sandbox/SandboxPage';
import { useResourceActions, type CreateAction } from './ResourceActions';
import { Button } from './ui/Button';
import { IconGoalOutline16, IconNewChatOutline16 } from './ui/icons';
import { CommandMenu, useCommandShortcut } from './shell/CommandMenu';
import { shellCommands } from './shell/commands';
import { setAppearance } from './theme/appearance';
import { useDocumentTitle } from './ui/useDocumentTitle';
import './Console.css';
import { useAgentAccess } from './resources/agentAccess';
import { SharedAgent } from './resources/SharedAgent';
import { SharedAgentDetails } from './resources/SharedAgentDetails';
import { AgentInventory } from './resources/AgentInventory';
import { ArtifactPublicationScope } from './resources/artifactRecovery';
import { SandboxAccessPage } from './resources/SandboxAccessPage';
import { WorkspaceContents } from './resources/WorkspaceContents';
import { scopeValue } from './resources/scope';
import { useNavigationScope } from './resources/NavigationScope';
import { useConsoleScope } from './resources/useConsoleScope';
import { OwnerViewsProvider, useOwnerViews } from './resources/retainedOwnerViews';

const EventGatewayDocs = lazy(() =>
  import('./services/EventGatewayDocs').then((module) => ({ default: module.EventGatewayDocs })),
);
const DocsIndex = lazy(() =>
  import('./services/EventGatewayDocs').then((module) => ({ default: module.DocsIndex })),
);

interface ConsoleProps {
  readonly user: User;
  readonly sidebarCollapsed?: boolean;
  readonly onSidebarCollapsedChange?: (collapsed: boolean) => void;
  readonly onServiceNavigate?: (route: AppRoute) => void;
  readonly route: AppRoute;
  readonly onSignOut: () => void;
  readonly onNavigate: (route: AppRoute) => void;
  readonly onCreate?: (action: CreateAction) => void;
  readonly hierarchyRevision?: number;
}

export function Console(props: ConsoleProps) {
  return (
    <OwnerViewsProvider key={props.user.id} viewerId={props.user.id}>
      <ConsoleContent {...props} />
    </OwnerViewsProvider>
  );
}

function ConsoleContent({
  user,
  route,
  onSignOut,
  onNavigate,
  onCreate,
  hierarchyRevision = 0,
  sidebarCollapsed: sharedCollapsed,
  onSidebarCollapsedChange,
  onServiceNavigate,
}: ConsoleProps) {
  const surfaceRef = useViewArrival(routePath(route), true, false);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  const [detailsOpen, setDetailsOpen] = useState(false);
  const [positions] = useState(() => new AgentViewPositions());
  const selectedAgent = route.kind === 'agent' ? route.agentId : undefined;
  const viewPosition = useMemo(() => positions.forAgent(selectedAgent), [positions, selectedAgent]);
  const [artifactPublicationScope] = useState(() => new ArtifactPublicationScope());
  const [revision, setRevision] = useState(0);
  const client = useCmaClient();
  const ownerViews = useOwnerViews();
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
  useEffect(() => {
    if (
      route.kind === 'agent' &&
      ((agentAccess.state.status === 'error' && !agentAccess.state.transient) ||
        (agentAccess.state.status === 'ready' &&
          agentAccess.state.value.detail.access === 'shared'))
    )
      ownerViews?.origins.delete(route.agentId);
  }, [route, agentAccess.state, ownerViews]);
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
    viewerId: user.id,
    suspended: accessUncertain,
    authorizedLineage: agentAccess.personalLineage,
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
  const newAgentUnavailable = !scopeReady
    ? 'Checking your access.'
    : navigationScope !== ''
      ? 'Switch to Personal to create an Agent.'
      : undefined;
  const canOpenTriggers = !user.visitor && user.capabilities.includes('publish_triggers');
  const [commandsOpen, setCommandsOpen] = useState(false);
  const toggleCommands = useCallback(() => setCommandsOpen((open) => !open), []);
  useCommandShortcut(toggleCommands, !user.visitor);
  useDocumentTitle(
    route.kind === 'home' || route.kind === 'agent_manager'
      ? 'Agent Manager'
      : route.kind === 'not_found'
        ? 'Page not found'
        : undefined,
  );
  const commands = useMemo(
    () =>
      shellCommands({
        items: navigation.status === 'ready' ? navigation.items : [],
        onNavigate,
        onNewAgent: canCreateInScope ? () => onCreate?.({ kind: 'new_agent' }) : undefined,
        canOpenTriggers,
        canOpenOperations: canOpenOperations(user.capabilities),
        onAppearance: setAppearance,
      }),
    [navigation, onNavigate, canCreateInScope, onCreate, canOpenTriggers, user.capabilities],
  );
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
  const content =
    route.kind === 'docs' ? (
      <Suspense fallback={<p role="status">Loading documentation</p>}>
        {route.view === 'event_gateway_protocol' ? (
          <EventGatewayDocs onNavigate={onNavigate} />
        ) : (
          <DocsIndex onNavigate={onNavigate} />
        )}
      </Suspense>
    ) : isBotRoute(route) ? (
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
        onNavigate={onNavigate}
      />
    ) : route.kind === 'agent' &&
      client.resourceViews !== undefined &&
      personalAgentId === undefined ? (
      <section
        className="console__empty"
        role={agentAccess.state.status === 'error' ? 'alert' : undefined}
      >
        <h1>{agentAccess.state.status === 'error' ? 'Agent unavailable' : 'Opening Agent'}</h1>
        {agentAccess.state.status === 'error' ? (
          <>
            <p>{agentAccess.state.message}</p>
            <Button onClick={agentAccess.reload}>Check access again</Button>
          </>
        ) : (
          <SkeletonRows rows={3} label="Opening Agent" />
        )}
      </section>
    ) : route.kind === 'agent' ? (
      <AgentSurface
        key={route.agentId}
        user={user}
        lifecycleClient={client}
        draft={draft}
        agentId={route.agentId}
        viewerId={user.id}
        controller={agent}
        viewPosition={viewPosition}
        accessUncertain={accessUncertain}
        accessAuthorized={agentAccess.personalReady && !accessUncertain}
        detailsOpen={detailsOpen}
        onDetailsOpenChange={setDetailsOpen}
        onNavigate={onNavigate}
        permissions={permissions}
        actionsBusy={actions.pending}
        onAgentAction={actions.select}
      />
    ) : route.kind === 'sandbox' ? (
      <SandboxAccessPage
        key={route.sandboxId}
        client={client.resourceViews}
        id={route.sandboxId}
        commandScope={sandboxCommandScope}
        onNavigate={onNavigate}
        personal={(accessUncertain) => (
          <SandboxPage
            key={route.sandboxId}
            client={client}
            sandboxId={route.sandboxId}
            viewerId={user.id}
            accessUncertain={accessUncertain}
            revision={revision}
            permissions={permissions}
            busy={
              actions.pending ||
              actions.busySandboxIds.has(route.sandboxId) ||
              actions.deletingSandboxIds.has(route.sandboxId)
            }
            onAction={actions.select}
            onNavigate={onNavigate}
          />
        )}
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
          {route.kind === 'home' || route.kind === 'agent_manager'
            ? 'Welcome to CMA'
            : route.kind.replaceAll('_', ' ')}
        </h1>
        {(route.kind === 'home' || route.kind === 'agent_manager') && canCreateInScope ? (
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
      phoneTitle="Agent Manager"
      detailsOpen={
        !accessUncertain && (personalAgentId !== undefined || shared !== undefined) && detailsOpen
      }
      compactDetailsOverlay
      details={
        route.kind === 'agent' && personalAgentId !== undefined && !accessUncertain ? (
          <AgentDetails
            key={route.agentId}
            owner={user.id}
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
      sidebarCollapsed={sharedCollapsed ?? sidebarCollapsed}
      onSidebarCollapsedChange={onSidebarCollapsedChange ?? setSidebarCollapsed}
      sidebar={({ collapsed, onToggle, onNavigate: onSidebarNavigate }) => (
        <Sidebar
          collapsed={collapsed}
          onToggleCollapsed={onToggle}
          route={route}
          onNavigate={(next) => {
            onSidebarNavigate();
            (onServiceNavigate ?? onNavigate)(next);
          }}
          newAgent={
            canCreate
              ? {
                  onCreate: () => onCreate?.({ kind: 'new_agent' }),
                  unavailableReason: newAgentUnavailable,
                }
              : undefined
          }
          onSearch={user.visitor ? undefined : () => setCommandsOpen(true)}
          links={
            canOpenTriggers
              ? [
                  {
                    id: 'triggers',
                    label: 'Triggers',
                    icon: <IconGoalOutline16 />,
                    route: { kind: 'triggers' },
                    current:
                      route.kind === 'triggers' ||
                      route.kind === 'trigger' ||
                      route.kind === 'trigger_embed',
                  },
                ]
              : []
          }
          onHome={() => {
            setDetailsOpen(false);
            onSidebarNavigate();
            onNavigate({ kind: 'home' });
          }}
          navigation={
            <ManagerNavigation
              client={scopeClient}
              scope={navigationScope}
              choices={scopeChoices}
              onScope={setNavigationScope}
              route={route}
              viewerId={user.id}
              navigation={navigation}
              currentAgent={ready?.detail}
              permissions={permissions}
              pending={actions.pending}
              busySandboxIds={actions.busySandboxIds}
              deletingSandboxIds={actions.deletingSandboxIds}
              onViewDeletion={actions.viewDeletion}
              onNavigate={(next) => {
                onSidebarNavigate();
                onNavigate(next);
              }}
              onAction={actions.select}
            />
          }
          account={
            <ProfileMenu
              user={user}
              collapsed={collapsed}
              onSignOut={onSignOut}
              onNavigate={(next) => {
                onSidebarNavigate();
                onNavigate(next);
              }}
            />
          }
        />
      )}
      center={
        <main className="console__surface" ref={surfaceRef}>
          <CommandMenu
            open={commandsOpen}
            onClose={() => setCommandsOpen(false)}
            commands={commands}
          />
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
