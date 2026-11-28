import { AgentAccessPending } from './agent/AgentAccessPending';
import { ComputerContext } from './agent/environment/context';
import { SkeletonRows } from './ui/Skeleton';
import { Documents, TriggerPublisherPage } from './ConsolePages';
import { BotManagement, isBotRoute } from './BotManagement';
import { Suspense, useCallback, useEffect, useMemo, useState } from 'react';
import { useCmaClient, useOperatorClient, usePublisherClient } from './api/context';
import { canOpenOperations } from './operator/capabilities';
import { AgentSurface } from './agent/AgentSurface';
import { AgentViewPositions } from './agent/viewPosition';
import { useAgentDraft } from './agent/draft';
import { useDraftAccess } from './agent/useDraftAccess';
import { clearOwnerDrafts } from './agent/draftStorage';
import { useIdentityUncertain } from './auth/context';
import { AgentSidePanel } from './agent/AgentSidePanel';
import { useAgentPanel } from './agent/useAgentPanel';
import { AgentComputerObservation } from './agent/environment/observation';
import { affectsCurrentAgent } from './agent/resourceChange';
import { useAgentController } from './agent/controller';
import type { ConsoleProps } from './ConsoleTypes';
import { ProfileMenu } from './profile/ProfileMenu';
import { LegacyProfileNotice } from './profile/LegacyProfileNotice';
import { routePath } from './routing/routes';
import { useViewArrival } from './ui/arrivalMotion';
import { DeliverablePreviewFrame as AppFrame } from './agent/DeliverablePreviewFrame';
import { Sidebar } from './shell/Sidebar';
import { ManagerNavigation, useEnvironmentNavigation } from './shell/ManagerNavigation';
import { navigationAgent, useNavigation } from './shell/navigation';
import { CharacterColorContext } from './character/characterColor';
import { SandboxPage } from './sandbox/SandboxPage';
import { useResourceActions } from './ResourceActions';
import { Button } from './ui/Button';
import { consoleLinks } from './ConsoleLinks';
import { ShellCommandMenu } from './shell/ShellCommandMenu';
import { ConsoleNotices, NotFoundView, UnavailableView } from './ConsoleViews';
import { ConsoleLanding } from './ConsoleLanding';
import { consoleCreationAccess } from './ConsoleCreationAccess';
import { useDocumentTitle } from './ui/useDocumentTitle';
import './Console.css';
import { useConsoleAgentAccess } from './resources/consoleAgentAccess';
import { useNavigationOpeningPreparation } from './agent/useOpeningPreparation';
import { SharedAgent } from './resources/SharedAgent';
import { AgentSharePage } from './resources/AgentSharePage';
import { AgentInventory } from './resources/AgentInventory';
import { ArtifactPublicationScope } from './resources/artifactRecovery';
import { SandboxAccessPage } from './resources/SandboxAccessPage';
import { WorkspaceAccessPage } from './resources/WorkspaceAccessPage';
import { scopeValue } from './resources/scope';
import { useNavigationScope } from './resources/NavigationScope';
import { useConsoleScope } from './resources/useConsoleScope';
import type { useDeletionNoticeReconciliation } from './useDeletionNoticeReconciliation';
import { DocsPage } from './services/DocsPage';
import { useReportOwner } from './usage/metrics/reportOwner';
import { OperationsRoute } from './operator/OperationsRoute';
export function ConsoleContent({
  settlements,
  selectedRetired: observedRetired,
  user,
  authEpoch = 0,
  visible = true,
  route,
  onSignOut,
  onNavigate,
  onCreate,
  hierarchyRevision = 0,
  sidebarCollapsed: sharedCollapsed,
  onSidebarCollapsedChange,
  onServiceNavigate,
}: ConsoleProps &
  Pick<ReturnType<typeof useDeletionNoticeReconciliation>, 'settlements' | 'selectedRetired'>) {
  const surfaceRef = useViewArrival(routePath(route), true, false);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  const selectedAgent = route.kind === 'agent' ? route.agentId : undefined;
  const panels = useAgentPanel(user.id, selectedAgent);
  const { panel, setPanel, setComputerOpen, detailsOpen, setDetailsOpen } = panels;
  const [positions] = useState(() => new AgentViewPositions());
  const viewPosition = useMemo(() => positions.forAgent(selectedAgent), [positions, selectedAgent]);
  const [artifactPublicationScope] = useState(() => new ArtifactPublicationScope());
  const [revision, setRevision] = useState(0);
  const client = useCmaClient();
  const computerObservation = useMemo(
    () =>
      new AgentComputerObservation(
        { client, subject: user.id, agentId: selectedAgent, authEpoch },
        () => setPanel('activity'),
        () => setPanel(undefined),
      ),
    [client, user.id, selectedAgent, setPanel, authEpoch],
  );
  const scopeClient = user.visitor ? undefined : client.resourceViews;
  const {
    value: navigationScope,
    select: setNavigationScope,
    ready: scopeReady,
  } = useConsoleScope(scopeClient, user.id, route);
  const scopeChoices = useNavigationScope(scopeClient, navigationScope);
  const draft = useAgentDraft(client, user.id, route.kind === 'agent' ? route.agentId : undefined);
  const sandboxCommandScope = useMemo(() => ({ client, viewerId: user.id }), [client, user.id]);
  const {
    agentAccess,
    accessUncertain: agentAccessUncertain,
    shared,
    personalAgentId,
    fileDownload,
  } = useConsoleAgentAccess(client, selectedAgent, user.id, authEpoch);
  const accessUncertain = useIdentityUncertain() || agentAccessUncertain;
  const selectedRetired =
    personalAgentId === selectedAgent && !accessUncertain ? observedRetired : undefined;
  const onAgentIntent = useNavigationOpeningPreparation(client, user.id, navigationScope === '');
  const operator = useOperatorClient();
  const publisher = usePublisherClient();
  const usagePrincipal = useReportOwner(user.id, user.capabilities, authEpoch);
  const navigation = useNavigation(client);
  const refreshNavigation = navigation.refresh;
  useEffect(refreshNavigation, [hierarchyRevision, refreshNavigation]);
  const refresh = useCallback(() => {
    refreshNavigation();
    setRevision((value) => value + 1);
  }, [refreshNavigation]);
  const located = navigationAgent(navigation, route.kind === 'agent' ? route.agentId : undefined);
  const agent = useAgentController(client, personalAgentId, {
    viewerId: user.id,
    authEpoch,
    selectedRetired,
    suspended: accessUncertain,
    authorizedLineage: agentAccess.personalLineage,
    summary: located?.agent,
    onHierarchyChange: refresh,
    onDeleted: () => {
      try {
        clearOwnerDrafts(user.id);
      } finally {
        onNavigate({ kind: 'home' });
      }
    },
  });
  const ready = agent.state.status === 'ready' ? agent.state : undefined;
  const deleted = ready?.detail.id === selectedAgent && ready?.detail.status === 'deleted';
  useDraftAccess(user.id, selectedAgent, agentAccess.state, deleted);
  useEffect(() => {
    if (!accessUncertain && agent.state.status === 'ready')
      agent.state.v2?.journal.reconcileDraft();
  }, [accessUncertain, agent.state, draft.recoveryBlocked]);
  const { canStartNewAgent, canCreateInScope, newAgentUnavailable, permissions } =
    consoleCreationAccess(user, navigationScope, scopeReady, accessUncertain, Boolean(onCreate));
  const actions = useResourceActions(
    client,
    (target) => {
      refresh();
      if (affectsCurrentAgent(target, agent.state)) agent.refreshDetail();
    },
    onCreate,
    user.id,
    onNavigate,
    !accessUncertain,
    permissions,
    settlements,
    { view: routePath(route), environment: route.kind === 'sandbox' },
  );
  const environments = useEnvironmentNavigation(
    navigation,
    actions.deletingSandboxIds,
    actions.unknownDeletingSandboxIds,
  );
  const canOpenTriggers = !user.visitor && user.capabilities.includes('publish_triggers');
  const [commandsOpen, setCommandsOpen] = useState(false);
  const newAgent = useCallback(() => onCreate?.({ kind: 'new_agent' }), [onCreate]);
  const showAgentPanel = !accessUncertain && !agent.retained && Boolean(personalAgentId ?? shared);
  useDocumentTitle(
    route.kind === 'home' || route.kind === 'agent_manager'
      ? 'Agent Manager'
      : route.kind === 'not_found'
        ? 'Page not found'
        : undefined,
    route.kind === 'agent' && (personalAgentId ?? shared) !== undefined ? 'inherit' : 'product',
  );
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
        authEpoch={accessUncertain ? undefined : authEpoch}
        viewerId={user.id}
        client={client.resourceViews}
        web={client}
        computerObservation={computerObservation}
        detail={shared}
        onAccessChanged={agentAccess.reload}
        filesOpen={panel !== undefined && panel !== 'details'}
        onFilesOpenChange={setComputerOpen}
        detailsOpen={detailsOpen}
        onDetailsOpenChange={setDetailsOpen}
        onNavigate={onNavigate}
      />
    ) : selectedAgent !== undefined && client.resourceViews && personalAgentId === undefined ? (
      <AgentAccessPending state={agentAccess.state} onRetry={agentAccess.reload} />
    ) : route.kind === 'agent' ? (
      <CharacterColorContext value={located?.sandbox.color}>
        <AgentSurface
          selectedRetired={selectedRetired}
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
          firstSendContext={{ authEpoch, visible: visible && !user.visitor }}
          filesOpen={panel !== undefined && panel !== 'details'}
          onFilesOpenChange={setComputerOpen}
          detailsOpen={detailsOpen}
          onDetailsOpenChange={setDetailsOpen}
          onNavigate={onNavigate}
          onPersonalHome={() => {
            setNavigationScope('');
            onNavigate({ kind: 'home' });
          }}
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
            client={client}
            sandboxId={route.sandboxId}
            viewerId={user.id}
            accessUncertain={accessUncertain}
            revision={revision}
            permissions={permissions}
            busy={actions.pending || actions.deletingSandboxIds.has(route.sandboxId)}
            lifecycleBusy={actions.busySandboxIds.has(route.sandboxId)}
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
      ['profiles', 'profile', 'marketplace', 'marketplace_profile'].includes(route.kind) ? (
      <LegacyProfileNotice />
    ) : route.kind === 'operations' ? (
      <OperationsRoute
        operator={operator}
        capabilities={user.capabilities}
        principal={usagePrincipal}
        bindingDiagnostics={user.bindingDiagnostics === true}
      />
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
        home={{
          ...environments.home,
          authEpoch,
          visible,
          navigation: scopeReady && navigationScope === '' ? navigation : undefined,
          scope: navigationScope,
          onRetry: refreshNavigation,
          onNavigate,
        }}
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
        detailsOpen={showAgentPanel && panel !== undefined}
        detailsExpanded={panels.computerExpanded}
        compactDetailsOverlay
        onDetailsClose={panels.dismissDetails}
        details={
          route.kind === 'agent' && showAgentPanel ? (
            <AgentSidePanel
              key={route.agentId}
              agentId={route.agentId}
              viewerId={user.id}
              shared={shared !== undefined}
              detail={ready?.detail}
              onStopForControl={agent.interruptForControl}
              resourceAvailability={shared === undefined ? agent.resourceAvailability : undefined}
              canManageSandbox={permissions.pauseSandbox}
              panel={panel}
              onSelect={setPanel}
              fullscreenFallback={panels}
              conversation={ready?.conversation}
              observation={computerObservation}
              onClose={() => setPanel(undefined)}
              {...fileDownload.sidePanel}
              publicationScope={artifactPublicationScope}
              historyLoading={ready?.historyLoading || !!ready?.historyUnavailable}
              controlReady={agent.controlBaselineReady}
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
                ref={environments.ref}
                authEpoch={authEpoch}
                selectedRetired={selectedRetired}
                attention={environments.attention}
                unknownDeletingSandboxIds={actions.unknownDeletingSandboxIds}
                client={scopeClient}
                scope={navigationScope}
                choices={scopeChoices}
                onScope={setNavigationScope}
                route={route}
                viewerId={user.id}
                navigation={navigation}
                currentAgent={ready?.detail}
                currentActivity={ready?.activityStatus}
                currentTurnId={
                  ready?.historyLoading || ready?.historyUnavailable
                    ? undefined
                    : ready?.conversation.latestTurnId
                }
                permissions={permissions}
                pending={actions.pending}
                busySandboxIds={actions.busySandboxIds}
                deletingSandboxIds={actions.deletingSandboxIds}
                onViewDeletion={actions.viewDeletion}
                deletionNotice={actions.environmentNotice}
                onNavigate={(next) => {
                  if (next.kind !== 'usage') onSidebarNavigate();
                  onNavigate(next);
                }}
                onRestoreTaskRoute={onNavigate}
                onAgentIntent={navigationScope === '' ? onAgentIntent : undefined}
                onAction={actions.select}
              />
            }
            account={
              <ProfileMenu
                user={user}
                collapsed={collapsed}
                onSignOut={onSignOut}
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
              onOpenEnvironments={() => environments.ref.current?.openEnvironments()}
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
            <ConsoleNotices error={actions.error} fileDownload={fileDownload} />
            {content}
          </main>
        }
      />
    </ComputerContext>
  );
}
