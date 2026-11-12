import { ComputerContext } from './agent/environment/context';
import { SkeletonRows } from './ui/Skeleton';
import { Documents, Operations, TriggerPublisherPage } from './ConsolePages';
import { BotManagement, isBotRoute } from './BotManagement';
import { Suspense, useCallback, useEffect, useMemo, useState } from 'react';
import { useCmaClient, useOperatorClient, usePublisherClient } from './api/context';
import { canOpenOperations } from './operator/capabilities';
import { AgentSurface } from './agent/AgentSurface';
import { AgentViewPositions } from './agent/viewPosition';
import { useAgentDraft } from './agent/draft';
import { AgentSidePanel } from './agent/AgentSidePanel';
import { useAgentPanel } from './agent/useAgentPanel';
import { AgentComputerObservation } from './agent/environment/observation';
import { useAgentController } from './agent/controller';
import type { ConsoleProps } from './ConsoleTypes';
import { ProfileMenu } from './profile/ProfileMenu';
import { LegacyProfileNotice } from './profile/LegacyProfileNotice';
import { routePath } from './routing/routes';
import { useViewArrival } from './ui/arrivalMotion';
import { DeliverablePreviewFrame as AppFrame } from './agent/DeliverablePreviewFrame';
import { Sidebar } from './shell/Sidebar';
import { ManagerNavigation } from './shell/ManagerNavigation';
import { navigationAgent, useNavigation } from './shell/navigation';
import { CharacterColorContext } from './character/characterColor';
import { SandboxPage } from './sandbox/SandboxPage';
import { useResourceActions } from './ResourceActions';
import { Button } from './ui/Button';
import { consoleLinks } from './ConsoleLinks';
import { ShellCommandMenu } from './shell/ShellCommandMenu';
import { ConsoleActionError, NotFoundView, UnavailableView } from './ConsoleViews';
import { ConsoleLanding } from './ConsoleLanding';
import { consoleCreationAccess } from './ConsoleCreationAccess';
import { useDocumentTitle } from './ui/useDocumentTitle';
import './Console.css';
import { useAgentAccess } from './resources/agentAccess';
import { SharedAgent } from './resources/SharedAgent';
import { AgentSharePage } from './resources/AgentSharePage';
import { AgentInventory } from './resources/AgentInventory';
import { ArtifactPublicationScope } from './resources/artifactRecovery';
import { SandboxAccessPage } from './resources/SandboxAccessPage';
import { WorkspaceAccessPage } from './resources/WorkspaceAccessPage';
import { scopeValue } from './resources/scope';
import { useNavigationScope } from './resources/NavigationScope';
import { useConsoleScope } from './resources/useConsoleScope';
import { OwnerViewsProvider, useOwnerViews } from './resources/retainedOwnerViews';
import { coalesceDetailReads } from './api/detailReads';
import { DocsPage } from './services/DocsPage';

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
  onDeleteAccount,
  onNavigate,
  onCreate,
  hierarchyRevision = 0,
  sidebarCollapsed: sharedCollapsed,
  onSidebarCollapsedChange,
  onServiceNavigate,
}: ConsoleProps) {
  const surfaceRef = useViewArrival(routePath(route), true, false);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  const { panel, setPanel, detailsOpen, setDetailsOpen } = useAgentPanel();
  const [positions] = useState(() => new AgentViewPositions());
  const selectedAgent = route.kind === 'agent' ? route.agentId : undefined;
  const viewPosition = useMemo(() => positions.forAgent(selectedAgent), [positions, selectedAgent]);
  const [artifactPublicationScope] = useState(() => new ArtifactPublicationScope());
  const [revision, setRevision] = useState(0);
  const sourceClient = useCmaClient();
  const reads = useMemo(() => coalesceDetailReads(sourceClient), [sourceClient]);
  useEffect(() => () => reads.dispose(), [reads]);
  const client = reads.client;
  const computerObservation = useMemo(
    () =>
      new AgentComputerObservation({ client, subject: user.id, agentId: selectedAgent }, () =>
        setPanel('activity'),
      ),
    [client, user.id, selectedAgent, setPanel],
  );
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
  const located = navigationAgent(navigation, route.kind === 'agent' ? route.agentId : undefined);
  const agent = useAgentController(client, personalAgentId, {
    viewerId: user.id,
    suspended: accessUncertain,
    authorizedLineage: agentAccess.personalLineage,
    summary: located?.agent,
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
  const { canStartNewAgent, canCreateInScope, newAgentUnavailable, permissions } =
    consoleCreationAccess(
      user,
      navigationScope,
      scopeReady,
      accessUncertain,
      onCreate !== undefined,
    );
  const canOpenTriggers = !user.visitor && user.capabilities.includes('publish_triggers');
  const [commandsOpen, setCommandsOpen] = useState(false);
  const newAgent = useCallback(() => onCreate?.({ kind: 'new_agent' }), [onCreate]);
  useDocumentTitle(
    route.kind === 'home' || route.kind === 'agent_manager'
      ? 'Agent Manager'
      : route.kind === 'not_found'
        ? 'Page not found'
        : undefined,
    route.kind === 'agent' && (personalAgentId !== undefined || shared !== undefined)
      ? 'inherit'
      : 'product',
  );
  const ready = agent.state.status === 'ready' ? agent.state : undefined;
  const content =
    route.kind === 'artifacts' || route.kind === 'document' ? (
      <Suspense fallback={<SkeletonRows rows={3} label="Opening Artifacts" />}>
        <Documents route={route} user={user} onNavigate={onNavigate} />
      </Suspense>
    ) : route.kind === 'docs' ? (
      <DocsPage view={route.view} onNavigate={onNavigate} />
    ) : isBotRoute(route) ? (
      <BotManagement user={user} route={route} onNavigate={onNavigate} />
    ) : route.kind === 'agent_share' ? (
      <AgentSharePage
        key={route.shareId}
        shareId={route.shareId}
        onScope={setNavigationScope}
        onRefresh={refreshNavigation}
        onNavigate={onNavigate}
      />
    ) : route.kind === 'inventory' ? (
      client.resourceViews === undefined ? (
        <UnavailableView
          title="Agent inventory unavailable"
          reason="This CMA deployment does not offer the Agents list. Your Agents are still listed in the sidebar."
        />
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
        viewerId={user.id}
        client={client.resourceViews}
        web={client}
        computerObservation={computerObservation}
        detail={shared}
        onAccessChanged={agentAccess.reload}
        filesOpen={panel !== undefined && panel !== 'details'}
        onFilesOpenChange={(open) => setPanel(open ? 'browser' : undefined)}
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
      <CharacterColorContext value={located?.sandbox.color}>
        <AgentSurface
          computerObservation={computerObservation}
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
          filesOpen={panel !== undefined && panel !== 'details'}
          onFilesOpenChange={(open) => setPanel(open ? 'browser' : undefined)}
          detailsOpen={detailsOpen}
          onDetailsOpenChange={setDetailsOpen}
          onNavigate={onNavigate}
          permissions={permissions}
          actionsBusy={actions.pending}
          onAgentAction={actions.select}
          onViewDeletionStatus={actions.viewDeletionStatus}
        />
      </CharacterColorContext>
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
      <WorkspaceAccessPage
        key={route.workspaceId}
        client={client}
        id={route.workspaceId}
        revision={revision}
        permissions={permissions}
        busy={actions.pending}
        onAction={actions.select}
        onNavigate={onNavigate}
      />
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
        <UnavailableView
          title="Operations unavailable"
          reason="Operations is for CMA operators. Ask your CMA administrator if you need access."
        />
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
        <UnavailableView
          title="Publishing unavailable"
          reason="Your account cannot publish Trigger links. Ask your CMA administrator for publishing access."
        />
      )
    ) : route.kind === 'not_found' ? (
      <NotFoundView />
    ) : (
      <ConsoleLanding
        client={client}
        user={user}
        route={route}
        canCreate={canCreateInScope}
        onCreated={(target) => {
          setRevision((value) => value + 1);
          onNavigate(target);
        }}
      />
    );
  return (
    <ComputerContext value={computerObservation}>
      <AppFrame
        previewScope={`${user.id}:${selectedAgent}:${accessUncertain}`}
        onPreviewOpen={() => setPanel(undefined)}
        phoneTitle="Agent Manager"
        taskLayout
        workspaceDetails={panel !== 'details'}
        detailsOpen={
          !accessUncertain &&
          (personalAgentId !== undefined || shared !== undefined) &&
          panel !== undefined
        }
        compactDetailsOverlay
        onDetailsClose={() => setPanel(undefined)}
        details={
          route.kind === 'agent' && !accessUncertain && (personalAgentId || shared) ? (
            <AgentSidePanel
              key={route.agentId}
              agentId={route.agentId}
              viewerId={user.id}
              shared={shared !== undefined}
              detail={ready?.detail}
              onStopForControl={agent.interruptForControl}
              resourceAvailability={agent.resourceAvailability}
              panel={panel}
              onSelect={setPanel}
              conversation={ready?.conversation}
              observation={computerObservation}
              onClose={() => setPanel(undefined)}
              onAccessChanged={agentAccess.reload}
              publicationScope={artifactPublicationScope}
              historyLoading={ready?.historyLoading || !!ready?.historyUnavailable}
              pending={agent.pending}
              onOpenSandbox={
                ready === undefined
                  ? undefined
                  : () => onNavigate({ kind: 'sandbox', sandboxId: ready.detail.sandbox.id })
              }
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
              canStartNewAgent
                ? {
                    current:
                      canCreateInScope && (route.kind === 'home' || route.kind === 'agent_manager'),
                    onCreate: () => onCreate?.({ kind: 'new_agent' }),
                    unavailableReason: newAgentUnavailable,
                  }
                : undefined
            }
            onSearch={user.visitor ? undefined : () => setCommandsOpen(true)}
            links={consoleLinks(
              route,
              canOpenTriggers,
              client.documents !== undefined && !user.visitor,
            )}
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
                  if (next.kind !== 'usage') onSidebarNavigate();
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
                onDeleteAccount={onDeleteAccount}
                onNavigate={(next) => {
                  if (next.kind !== 'usage') onSidebarNavigate();
                  onNavigate(next);
                }}
              />
            }
          />
        )}
        center={
          <main className="console__surface" ref={surfaceRef}>
            <ShellCommandMenu
              open={commandsOpen}
              onOpenChange={setCommandsOpen}
              enabled={!user.visitor}
              navigation={navigation}
              onNavigate={onNavigate}
              onNewAgent={canCreateInScope ? newAgent : undefined}
              canOpenTriggers={canOpenTriggers}
              canOpenOperations={canOpenOperations(user.capabilities)}
            />
            {actions.notice}
            {route.kind !== 'agent' && actions.viewDeletionStatus ? (
              <Button size="sm" onClick={actions.viewDeletionStatus}>
                View deletion status
              </Button>
            ) : null}
            <ConsoleActionError error={actions.error} />
            {content}
          </main>
        }
      />
    </ComputerContext>
  );
}
