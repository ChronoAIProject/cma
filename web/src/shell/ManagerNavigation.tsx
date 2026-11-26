import { SidebarContext } from './sidebarContext';
import {
  useContext,
  useImperativeHandle,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
  type Ref,
} from 'react';
import { RetiredAgentRow } from './PersonalAgentTree';
import { DeletionProgressList } from './DeletionProgressList';
import { runtimeAttention, type RuntimeAttention } from './runtimeAttention';
import { SkeletonRows } from '../ui/Skeleton';
import type { AgentDetail, SandboxSummary } from '../api/types';
import type { AppRoute } from '../routing/routes';
import type { ResourceViewsClient } from '../resources/client';
import {
  NavigationScopePicker,
  useNavigationScope,
  type NavigationScopeChoices,
} from '../resources/NavigationScope';
import { ScopedNavigation } from '../resources/ScopedNavigation';
import { Button } from '../ui/Button';
import { sameAgent } from '../agent/status';
import type { ActivityStatus } from '../agent/activityStatus';
import { Tree, type TreeAction, type TreePermissions } from './Tree';
import type { NavigationState } from './navigation';
import { NavigationFilters, type NavigationView } from './NavigationFilters';
import { TreeCreationScope } from './treeCreationScope';
import { useNavigationFilterState } from './navigationFilterState';

export interface EnvironmentNavigationHandle {
  openEnvironments(options?: {
    readonly scope?: 'personal';
    readonly sandboxId?: string;
    readonly status?: 'failed';
  }): void;
}

export function useEnvironmentNavigation(
  navigation: NavigationState,
  accepted: ReadonlySet<string>,
  unknown: ReadonlySet<string>,
) {
  const ref = useRef<EnvironmentNavigationHandle>(null);
  const attention = runtimeAttention(navigation, { accepted, unknown });
  return {
    ref,
    attention,
    home: {
      attention,
      onEnvironments: () => ref.current!.openEnvironments({ scope: 'personal' }),
      onReviewRuntime: () =>
        ref.current!.openEnvironments({
          scope: 'personal',
          status: 'failed',
          sandboxId:
            attention.failedSandboxes.length === 1
              ? attention.failedSandboxes[0]?.sandboxId
              : undefined,
        }),
    },
  };
}

export function ManagerNavigation(props: Parameters<typeof ViewerNavigation>[0]) {
  return <ViewerNavigation key={`${props.viewerId}:${props.authEpoch ?? 0}`} {...props} />;
}

function ViewerNavigation({
  ref,
  authEpoch = 0,
  selectedRetired,
  attention,
  unknownDeletingSandboxIds,
  client,
  scope,
  choices,
  route,
  viewerId,
  navigation,
  currentAgent,
  currentActivity,
  currentTurnId,
  permissions,
  pending,
  busySandboxIds,
  deletingSandboxIds,
  onScope,
  onViewDeletion,
  deletionNotice,
  onNavigate,
  onRestoreTaskRoute = onNavigate,
  onAgentIntent,
  onAction,
}: {
  readonly ref?: Ref<EnvironmentNavigationHandle>;
  readonly authEpoch?: number;
  readonly selectedRetired?: {
    readonly detail: AgentDetail;
    readonly viewerId: string;
    readonly authEpoch: number;
  };
  readonly attention?: RuntimeAttention;
  readonly unknownDeletingSandboxIds?: ReadonlySet<string>;
  readonly client?: ResourceViewsClient;
  readonly scope: string;
  readonly choices?: NavigationScopeChoices;
  readonly route: AppRoute;
  readonly viewerId: string;
  readonly navigation: NavigationState & { refresh: () => void };
  readonly currentAgent?: AgentDetail;
  readonly currentActivity?: ActivityStatus;
  readonly currentTurnId?: string;
  readonly permissions: TreePermissions;
  readonly pending: boolean;
  readonly busySandboxIds: ReadonlySet<string>;
  readonly deletingSandboxIds: ReadonlySet<string>;
  readonly onScope: (scope: string) => void;
  readonly onViewDeletion: (id: string) => void;
  readonly deletionNotice?: ReactNode;
  readonly onNavigate: (route: AppRoute) => void;
  readonly onRestoreTaskRoute?: (route: AppRoute) => void;
  readonly onAgentIntent?: (agentId: string, active: boolean) => void;
  readonly onAction: (action: TreeAction) => void;
}) {
  const [view, setView] = useState<NavigationView>('agent');
  const [environmentScope, setEnvironmentScope] = useState('');
  const sidebar = useContext(SidebarContext);
  const [filtersOpen, setFiltersOpen] = useState(false);
  const [environmentsOpened, setEnvironmentsOpened] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const scrollport = useRef<HTMLDivElement>(null);
  const taskScrollTop = useRef(0);
  const taskPanel = useRef<HTMLDivElement>(null);
  const taskFocus = useRef<HTMLElement | null>(null);
  const activeScope = view === 'sandbox' ? environmentScope : scope;
  const localChoices = useNavigationScope(choices === undefined ? client : undefined, activeScope);
  const scopeChoices = choices ?? localChoices;
  const filters = useNavigationFilterState(viewerId, activeScope);
  const { statuses } = filters;
  const [taskReturn, setTaskReturn] = useState({
    route,
    scope,
    scrollTop: 0,
    status: statuses.agent,
  });
  const modeChanged = useRef(false);
  const refreshRequestedFor = useRef<string | undefined>(undefined);
  const status = statuses[view];
  function selectScope(next: string) {
    if (view === 'sandbox') setEnvironmentScope(next);
    onScope(next);
  }
  function selectView(next: NavigationView, nextEnvironmentScope = environmentScope) {
    if (sidebar?.collapsed) {
      modeChanged.current = true;
      sidebar.toggle();
    }
    if (view === next) return;
    modeChanged.current = true;
    if (next === 'sandbox') {
      setTaskReturn({
        route,
        scope,
        status: statuses.agent,
        scrollTop: sidebar?.collapsed
          ? taskScrollTop.current
          : (scrollport.current?.scrollTop ?? 0),
      });
      setFiltersOpen(false);
      setEnvironmentsOpened(true);
      if (scope !== nextEnvironmentScope) onScope(nextEnvironmentScope);
    } else {
      if (scope !== taskReturn.scope) onScope(taskReturn.scope);
      if (route !== taskReturn.route) onRestoreTaskRoute(taskReturn.route);
    }
    setView(next);
  }
  useImperativeHandle(ref, () => ({
    openEnvironments(options) {
      const targetScope = options?.scope === 'personal' ? '' : environmentScope;
      if (targetScope !== environmentScope) setEnvironmentScope(targetScope);
      selectView('sandbox', targetScope);
      if (view === 'sandbox' && scope !== targetScope) onScope(targetScope);
      if (options?.status) filters.select('sandbox', options.status, targetScope);
      if (options?.sandboxId) onNavigate({ kind: 'sandbox', sandboxId: options.sandboxId });
    },
  }));
  useLayoutEffect(() => {
    if (currentAgent === undefined || navigation.status !== 'ready') return;
    const summary = navigation.items
      .flatMap((item) => item.workspaces)
      .flatMap((workspace) => workspace.agents)
      .find((agent) => agent.id === currentAgent.id);
    if (summary === undefined || summary.updatedAt === currentAgent.updatedAt) {
      refreshRequestedFor.current = undefined;
      return;
    }
    const request = `${currentAgent.id}:${currentAgent.updatedAt}`;
    if (refreshRequestedFor.current === request) return;
    refreshRequestedFor.current = request;
    navigation.refresh();
  }, [currentAgent, navigation]);
  useLayoutEffect(() => {
    if (!modeChanged.current || sidebar?.collapsed) return;
    modeChanged.current = false;
    const mode = root.current?.querySelector<HTMLButtonElement>(`[data-navigation-mode="${view}"]`);
    const task = taskPanel.current?.contains(taskFocus.current)
      ? taskFocus.current
      : taskPanel.current?.querySelector<HTMLElement>('[role="treeitem"][aria-selected="true"]');
    (view === 'agent' ? (task ?? mode) : mode)?.focus({ preventScroll: true });
    if (scrollport.current)
      scrollport.current.scrollTop = view === 'agent' ? taskReturn.scrollTop : 0;
  }, [view, sidebar?.collapsed, taskReturn.scrollTop]);
  const retired =
    scope === '' &&
    selectedRetired?.viewerId === viewerId &&
    selectedRetired.authEpoch === authEpoch &&
    route.kind === 'agent' &&
    route.agentId === selectedRetired.detail.id &&
    selectedRetired.detail.sandbox.status === 'retired' &&
    selectedRetired.detail.worktree.agentId === selectedRetired.detail.id &&
    selectedRetired.detail.worktree.workspaceId === selectedRetired.detail.workspace.id &&
    selectedRetired.detail.worktree.sandboxId === selectedRetired.detail.sandbox.id
      ? selectedRetired.detail
      : undefined;
  const observedAttention =
    attention ??
    runtimeAttention(navigation, {
      accepted: deletingSandboxIds,
      unknown: unknownDeletingSandboxIds ?? new Set(),
    });
  const deletions = new Map<
    string,
    Pick<SandboxSummary, 'id' | 'name'> &
      Partial<Pick<SandboxSummary, 'status' | 'lifecycle'>> & { unknown?: boolean }
  >();
  if (scope === '') {
    for (const id of deletingSandboxIds) deletions.set(id, { id, name: 'Environment' });
    for (const id of unknownDeletingSandboxIds ?? [])
      if (!deletions.has(id)) deletions.set(id, { id, name: 'Environment', unknown: true });
    if (navigation.status === 'ready')
      for (const { sandbox } of navigation.items) {
        if (sandbox.status === 'retiring' || deletions.has(sandbox.id))
          deletions.set(sandbox.id, {
            ...deletions.get(sandbox.id),
            id: sandbox.id,
            name: sandbox.name,
            status: sandbox.status,
            lifecycle: navigation.refreshError ? undefined : sandbox.lifecycle,
            unknown: sandbox.status === 'retiring' ? false : deletions.get(sandbox.id)?.unknown,
          });
        if (
          !navigation.refreshError &&
          sandbox.status === 'retired' &&
          !deletions.get(sandbox.id)?.unknown
        )
          deletions.delete(sandbox.id);
      }
  }
  const navigationPanel = (panelView: NavigationView) => (
    <ScopedNavigation
      route={panelView === 'agent' && view === 'sandbox' ? taskReturn.route : route}
      client={client}
      scope={
        panelView === 'sandbox' ? environmentScope : view === 'sandbox' ? taskReturn.scope : scope
      }
      choices={scopeChoices}
      view={panelView}
      statusFilter={
        panelView === 'agent' && view === 'sandbox' ? taskReturn.status : statuses[panelView]
      }
      onNavigate={onNavigate}
      personal={
        <>
          {navigation.status === 'ready' ? (
            <>
              {navigation.refreshError ? (
                <Button size="sm" onClick={navigation.refresh}>
                  Try again
                </Button>
              ) : null}
              <Tree
                selectedRetired={retired}
                onShowAll={() => filters.select('agent', 'all')}
                attention={observedAttention}
                showDeletionProgress={false}
                currentTask={
                  currentAgent && (currentActivity !== undefined || currentTurnId !== undefined)
                    ? { agent: currentAgent, status: currentActivity, turnId: currentTurnId }
                    : undefined
                }
                active={panelView === view}
                resourceClient={client}
                viewerId={viewerId}
                view={panelView}
                statusFilter={
                  panelView === 'agent' && view === 'sandbox'
                    ? taskReturn.status
                    : statuses[panelView]
                }
                items={navigation.items.map((item) => ({
                  ...item,
                  workspaces: item.workspaces.map((workspace) => ({
                    ...workspace,
                    agents: workspace.agents.map((summary) =>
                      (() => {
                        const current =
                          currentAgent !== undefined && sameAgent(currentAgent, summary)
                            ? currentAgent
                            : undefined;
                        return current
                          ? {
                              ...summary,
                              status: current.status,
                              title: current.title,
                              ownerTitle: current.ownerTitle,
                              runtimeMetadata: current.runtimeMetadata,
                              taskStatus: summary.taskStatus,
                            }
                          : summary;
                      })(),
                    ),
                  })),
                }))}
                refreshError={navigation.refreshError}
                lifecycleBusy={pending}
                busySandboxIds={busySandboxIds}
                deletingSandboxIds={deletingSandboxIds}
                onViewDeletion={onViewDeletion}
                route={route}
                permissions={permissions}
                onNavigate={onNavigate}
                onAgentIntent={onAgentIntent}
                onAction={onAction}
              />
            </>
          ) : navigation.status === 'loading' ? (
            <SkeletonRows rows={4} label="Loading tasks" />
          ) : (
            <div className="console__navigation-status" role="alert">
              <p>{navigation.message}</p>
              <Button onClick={navigation.refresh}>Try again</Button>
            </div>
          )}
          {panelView === 'agent' && navigation.status !== 'ready' && retired ? (
            <div role="tree" aria-label="Personal Agents" className="agent-navigation">
              <RetiredAgentRow detail={retired} onNavigate={onNavigate} />
            </div>
          ) : null}
        </>
      }
    />
  );
  return (
    <div ref={root} className="manager-navigation">
      <NavigationFilters
        scope={
          client === undefined ? undefined : (
            <NavigationScopePicker
              choices={scopeChoices}
              value={activeScope}
              onChange={selectScope}
            />
          )
        }
        scopeSummary={
          client === undefined
            ? undefined
            : (scopeChoices.options.find((option) => option.value === activeScope)?.label ??
              'Personal')
        }
        scopeActive={client !== undefined && activeScope !== ''}
        view={view}
        onView={selectView}
        open={filtersOpen}
        onOpenChange={setFiltersOpen}
        status={status}
        onStatus={(next) => filters.select(view, next)}
        onClear={() => {
          filters.clear();
          if (activeScope !== '') selectScope('');
        }}
      />
      {filters.unavailable ? (
        <p className="resource-tree__notice" role="status">
          Filters apply now but cannot be remembered after refresh.
        </p>
      ) : null}
      {view === 'sandbox' && !sidebar?.collapsed ? deletionNotice : null}
      <div
        ref={scrollport}
        className="manager-navigation__list"
        onScroll={(event) => {
          if (view === 'agent' && !sidebar?.collapsed)
            taskScrollTop.current = event.currentTarget.scrollTop;
        }}
      >
        <TreeCreationScope viewerId={viewerId}>
          <div
            ref={taskPanel}
            hidden={view !== 'agent'}
            onFocusCapture={(event) => {
              const row = (event.target as HTMLElement).closest<HTMLElement>('[role="treeitem"]');
              if (row) taskFocus.current = row;
            }}
          >
            {navigationPanel('agent')}
          </div>
          {environmentsOpened ? (
            <div hidden={view !== 'sandbox'}>{navigationPanel('sandbox')}</div>
          ) : null}
        </TreeCreationScope>
      </div>
      <DeletionProgressList
        footer
        sandboxes={[...deletions.values()]}
        onViewStatus={onViewDeletion}
      />
    </div>
  );
}
