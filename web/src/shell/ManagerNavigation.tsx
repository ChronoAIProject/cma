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
import { Tree, type TreeAction, type TreePermissions } from './Tree';
import type { NavigationState } from './navigation';
import { NavigationFilters, type NavigationView } from './NavigationFilters';
import { Menu } from '../ui/Menu';
import { IconSettingsOutline16 } from '../ui/icons';
import { TreeCreationScope } from './treeCreationScope';

export function ManagerNavigation({
  client,
  scope,
  choices,
  route,
  viewerId,
  navigation,
  currentAgent,
  permissions,
  pending,
  busySandboxIds,
  deletingSandboxIds,
  onScope,
  onViewDeletion,
  onNavigate,
  onAction,
}: {
  readonly client?: ResourceViewsClient;
  readonly scope: string;
  readonly choices: NavigationScopeChoices;
  readonly route: AppRoute;
  readonly viewerId: string;
  readonly navigation: NavigationState & { refresh: () => void };
  readonly currentAgent?: AgentDetail;
  readonly permissions: TreePermissions;
  readonly pending: boolean;
  readonly busySandboxIds: ReadonlySet<string>;
  readonly deletingSandboxIds: ReadonlySet<string>;
  readonly onScope: (scope: string) => void;
  readonly onViewDeletion: (id: string) => void;
  readonly onNavigate: (route: AppRoute) => void;
  readonly onAction: (action: TreeAction) => void;
}) {
  const [view, setView] = useState<NavigationView>('agent');
  const settingsHost = useContext(NavigationSettingsContext);
  const sidebar = useContext(SidebarContext);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [environmentsOpened, setEnvironmentsOpened] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const taskFocus = useRef<HTMLElement | null>(null);
  const [taskReturn, setTaskReturn] = useState({ route, scope, scrollTop: 0 });
  const modeChanged = useRef(false);
  const refreshRequestedFor = useRef<string | undefined>(undefined);
  const [statuses, setStatuses] = useState({ agent: 'all', sandbox: 'all' });
  const status = statuses[view];
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
    if (!modeChanged.current) return;
    if (view === 'sandbox')
      root.current?.querySelector<HTMLButtonElement>('[data-back-to-tasks]')?.focus();
    else {
      const target = taskFocus.current?.isConnected
        ? taskFocus.current
        : root.current?.querySelector<HTMLElement>('[role="treeitem"][aria-selected="true"]');
      target?.focus({ preventScroll: true });
      const scrollport = root.current?.closest('.sidebar__navigation');
      if (scrollport) scrollport.scrollTop = taskReturn.scrollTop;
    }
  }, [view, taskReturn.scrollTop]);
  const navigationPanel = (panelView: NavigationView) => (
    <ScopedNavigation
      route={panelView === 'agent' && view === 'sandbox' ? taskReturn.route : route}
      client={client}
      scope={panelView === 'agent' && view === 'sandbox' ? taskReturn.scope : scope}
      choices={choices}
      view={panelView}
      statusFilter={statuses[panelView]}
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
              active={panelView === view}
              resourceClient={client}
              viewerId={viewerId}
              view={panelView}
              statusFilter={statuses[panelView]}
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
    <div ref={root}>
      {settingsHost
        ? createPortal(
            <Menu
              open={settingsOpen}
              label="Navigation settings"
              items={[{ id: 'environments', label: 'Manage environments' }]}
              onClose={() => setSettingsOpen(false)}
              onSelect={() => {
                if (sidebar?.collapsed) sidebar.toggle();
                if (view === 'sandbox') {
                  setSettingsOpen(false);
                  return;
                }
                setTaskReturn({
                  route,
                  scope,
                  scrollTop: root.current?.closest('.sidebar__navigation')?.scrollTop ?? 0,
                });
                modeChanged.current = true;
                setSettingsOpen(false);
                setEnvironmentsOpened(true);
                setView('sandbox');
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
      {view === 'sandbox' ? (
        <button
          type="button"
          className="resource-tree__heading"
          data-back-to-tasks
          onClick={() => {
            modeChanged.current = true;
            setView('agent');
            if (scope !== taskReturn.scope) onScope(taskReturn.scope);
            if (route !== taskReturn.route) onNavigate(taskReturn.route);
          }}
        >
          Back to tasks
        </button>
      ) : null}
      <NavigationFilters
        scope={
          client === undefined ? undefined : (
            <NavigationScopePicker choices={choices} value={scope} onChange={onScope} />
          )
        }
        onManageEnvironments={
          view === 'agent'
            ? () => {
                setTaskReturn({
                  route,
                  scope,
                  scrollTop: root.current?.closest('.sidebar__navigation')?.scrollTop ?? 0,
                });
                modeChanged.current = true;
                setEnvironmentsOpened(true);
                setView('sandbox');
              }
            : undefined
        }
        scopeSummary={
          client === undefined
            ? undefined
            : (choices.options.find((option) => option.value === scope)?.label ?? 'Personal')
        }
        scopeActive={client !== undefined && scope !== ''}
        view={view}
        status={status}
        onStatus={(next) => setStatuses((previous) => ({ ...previous, [view]: next }))}
        onClear={() => {
          setStatuses({ agent: 'all', sandbox: 'all' });
          if (scope !== '') onScope('');
        }}
      />
      <TreeCreationScope viewerId={viewerId}>
        <div
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
  );
}
