import { Presence } from './ui/Presence';
import { BridgeEntry } from './bridges/BridgeEntry';
import { lazy, Suspense, useCallback, useEffect, useMemo, useState } from 'react';
import { LandingCharacter } from './landing/LandingCharacter';
import {
  useCmaClient,
  useProfileViewsClient,
  useOperatorClient,
  usePublisherClient,
} from './api/context';
import { canOpenOperations } from './operator/capabilities';
import { AgentSurface } from './agent/AgentSurface';
import { useAgentDraft } from './agent/draft';
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
import { useAgentAccess } from './resources/agentAccess';
import { SharedAgent } from './resources/SharedAgent';
import { AgentInventory } from './resources/AgentInventory';
import { SharingButton } from './resources/SharingButton';
import { ArtifactButton } from './resources/ArtifactButton';
import { ArtifactPublicationScope } from './resources/artifactRecovery';
import { ChannelUsage } from './resources/ChannelUsage';
import { SandboxAccessPage } from './resources/SandboxAccessPage';
import { WorkspaceContents } from './resources/WorkspaceContents';
import { scopeValue } from './resources/scope';
import { ScopedNavigation } from './resources/ScopedNavigation';

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
  const [navigationScope, setNavigationScope] = useState(
    route.kind === 'inventory'
      ? scopeValue(route.organization, route.shared)
      : route.kind === 'bridges' || route.kind === 'bridge_new'
        ? scopeValue(route.organization)
        : '',
  );
  const [detailsOpen, setDetailsOpen] = useState(false);
  const [artifactPublicationScope] = useState(() => new ArtifactPublicationScope());
  const [revision, setRevision] = useState(0);
  const client = useCmaClient();
  const draft = useAgentDraft(client, user.id, route.kind === 'agent' ? route.agentId : undefined);
  const sandboxCommandScope = useMemo(() => ({ client, viewerId: user.id }), [client, user.id]);
  const agentAccess = useAgentAccess(
    client.resourceViews,
    route.kind === 'agent' ? route.agentId : undefined,
  );
  const shared =
    agentAccess.state.status === 'ready' && agentAccess.state.value.detail.access === 'shared'
      ? agentAccess.state.value.detail
      : undefined;
  const personalAgentId =
    route.kind === 'agent' && (client.resourceViews === undefined || agentAccess.personalReady)
      ? route.agentId
      : undefined;
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
  const agent = useAgentController(client, personalAgentId, {
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
    exportHistory: canUse,
    deleteSandbox: user.capabilities.includes('delete_own_sandbox'),
  };
  const ready = agent.state.status === 'ready' ? agent.state : undefined;
  const content =
    route.kind === 'bridges' || route.kind === 'bridge' || route.kind === 'bridge_new' ? (
      <BridgeEntry
        client={user.visitor ? undefined : client.bridgeViews}
        viewerId={user.id}
        route={route}
        onNavigate={(next) => {
          if (next.kind === 'bridges' || next.kind === 'bridge_new')
            setNavigationScope(scopeValue(next.organization));
          onNavigate(next);
        }}
      />
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
        publicationScope={artifactPublicationScope}
        key={shared.id}
        client={client.resourceViews}
        web={client}
        detail={shared}
        onAccessChanged={agentAccess.reload}
      />
    ) : route.kind === 'agent' &&
      client.resourceViews !== undefined &&
      personalAgentId === undefined &&
      agentAccess.state.status !== 'ready' ? (
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
        draft={draft}
        agentId={route.agentId}
        viewerId={user.id}
        controller={agent}
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
            busy={actions.pending}
            onAction={actions.select}
            onNavigate={onNavigate}
          />
        }
      />
    ) : route.kind === 'workspace' ? (
      <section className="shared-hierarchy">
        <header>
          <h1>Workspace</h1>
        </header>
        <div className="shared-hierarchy__body">
          {client.resourceViews === undefined ? (
            <p>Workspace unavailable</p>
          ) : (
            <WorkspaceContents
              key={route.workspaceId}
              client={client.resourceViews}
              id={route.workspaceId}
              onNavigate={onNavigate}
            />
          )}
        </div>
      </section>
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
      detailsOpen={personalAgentId !== undefined && detailsOpen}
      compactDetailsOverlay
      details={
        route.kind === 'agent' && personalAgentId !== undefined ? (
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
            channelUsage={
              detailsOpen && ready !== undefined && client.resourceViews !== undefined ? (
                <ChannelUsage
                  key={ready.detail.id}
                  client={client.resourceViews}
                  id={ready.detail.id}
                />
              ) : undefined
            }
            artifacts={
              detailsOpen && ready !== undefined && client.resourceViews !== undefined ? (
                <ArtifactButton
                  publicationScope={artifactPublicationScope}
                  key={ready.detail.id}
                  client={client.resourceViews}
                  id={ready.detail.id}
                />
              ) : undefined
            }
            sharing={
              detailsOpen && ready !== undefined && client.resourceViews !== undefined ? (
                <SharingButton
                  client={client.resourceViews}
                  target={{
                    kind: 'sandbox',
                    id: ready.detail.sandbox.id,
                    name: ready.detail.sandbox.name,
                  }}
                />
              ) : undefined
            }
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
            <ScopedNavigation
              client={user.visitor ? undefined : client.resourceViews}
              scope={navigationScope}
              onScopeChange={setNavigationScope}
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
              collapsed={collapsed}
              onSignOut={onSignOut}
              onNavigate={onNavigate}
              canManageBridges={!user.visitor && client.bridgeViews !== undefined}
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
