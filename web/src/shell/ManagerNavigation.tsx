import { createPortal } from 'react-dom';
import { NavigationSettingsContext, SidebarContext } from './sidebarContext';
import { useContext, useLayoutEffect, useRef, useState } from 'react';
import { SkeletonRows } from '../ui/Skeleton';
import type { AgentDetail } from '../api/types';
import type { AppRoute } from '../routing/routes';
import type { ResourceViewsClient } from '../resources/client';
import { NavigationScopePicker, type NavigationScopeChoices } from '../resources/NavigationScope';
import { ScopedNavigation } from '../resources/ScopedNavigation';
import { Button } from '../ui/Button';
import { sameAgent } from '../agent/status';
import type { ActivityStatus } from '../agent/activityStatus';
import { Tree, type TreeAction, type TreePermissions } from './Tree';
import type { NavigationState } from './navigation';
import { NavigationFilters, type NavigationView } from './NavigationFilters';
import { Menu } from '../ui/Menu';
import { IconSettingsOutline16 } from '../ui/icons';
import { TreeCreationScope } from './treeCreationScope';
import { useNavigationFilterState } from './navigationFilterState';

export function ManagerNavigation(props: Parameters<typeof ViewerNavigation>[0]) {
  return <ViewerNavigation key={props.viewerId} {...props} />;
}

function ViewerNavigation({
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
  onNavigate,
  onRestoreTaskRoute = onNavigate,
  onAgentIntent,
  onAction,
}: {
  readonly client?: ResourceViewsClient;
  readonly scope: string;
  readonly choices: NavigationScopeChoices;
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
  readonly onNavigate: (route: AppRoute) => void;
  readonly onRestoreTaskRoute?: (route: AppRoute) => void;
  readonly onAgentIntent?: (agentId: string, active: boolean) => void;
  readonly onAction: (action: TreeAction) => void;
}) {
  const [view, setView] = useState<NavigationView>('agent');
  const settingsHost = useContext(NavigationSettingsContext);
  const sidebar = useContext(SidebarContext);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [filtersOpen, setFiltersOpen] = useState(false);
  const [environmentsOpened, setEnvironmentsOpened] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const scrollport = useRef<HTMLDivElement>(null);
  const taskScrollTop = useRef(0);
  const taskPanel = useRef<HTMLDivElement>(null);
  const taskFocus = useRef<HTMLElement | null>(null);
  const filters = useNavigationFilterState(viewerId, scope);
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
  function selectView(next: NavigationView) {
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
    } else {
      if (scope !== taskReturn.scope) onScope(taskReturn.scope);
      if (route !== taskReturn.route) onRestoreTaskRoute(taskReturn.route);
    }
    setView(next);
  }
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
  const navigationPanel = (panelView: NavigationView) => (
    <ScopedNavigation
      route={panelView === 'agent' && view === 'sandbox' ? taskReturn.route : route}
      client={client}
      scope={panelView === 'agent' && view === 'sandbox' ? taskReturn.scope : scope}
      choices={choices}
      view={panelView}
      statusFilter={
        panelView === 'agent' && view === 'sandbox' ? taskReturn.status : statuses[panelView]
      }
      onNavigate={onNavigate}
      personal={
        navigation.status === 'ready' ? (
          <>
            {navigation.refreshError ? (
              <Button size="sm" onClick={navigation.refresh}>
                Try again
              </Button>
            ) : null}
            <Tree
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
        )
      }
    />
  );
  return (
    <div ref={root} className="manager-navigation">
      {settingsHost
        ? createPortal(
            <Menu
              open={settingsOpen}
              label="Navigation settings"
              items={[{ id: 'environments', label: 'Manage environments' }]}
              onClose={() => setSettingsOpen(false)}
              onSelect={() => {
                setSettingsOpen(false);
                selectView('sandbox');
              }}
              anchor={
                <button
                  type="button"
                  className="sidebar__toggle"
                  aria-label="Settings"
                  title="Settings"
                  aria-haspopup="menu"
                  aria-expanded={settingsOpen}
                  onClick={() => setSettingsOpen((open) => !open)}
                >
                  <IconSettingsOutline16 />
                </button>
              }
            />,
            settingsHost,
          )
        : null}
      <NavigationFilters
        scope={
          client === undefined ? undefined : (
            <NavigationScopePicker choices={choices} value={scope} onChange={onScope} />
          )
        }
        scopeSummary={
          client === undefined
            ? undefined
            : (choices.options.find((option) => option.value === scope)?.label ?? 'Personal')
        }
        scopeActive={client !== undefined && scope !== ''}
        view={view}
        onView={selectView}
        open={filtersOpen}
        onOpenChange={setFiltersOpen}
        status={status}
        onStatus={(next) => filters.select(view, next)}
        onClear={() => {
          filters.clear();
          if (scope !== '') onScope('');
        }}
      />
      {filters.unavailable ? (
        <p className="resource-tree__notice" role="status">
          Filters apply now but cannot be remembered after refresh.
        </p>
      ) : null}
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
    </div>
  );
}
