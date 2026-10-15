import { useState } from 'react';
import type { AgentDetail } from '../api/types';
import type { AppRoute } from '../routing/routes';
import type { ResourceViewsClient } from '../resources/client';
import type { NavigationScopeChoices } from '../resources/NavigationScope';
import { ScopedNavigation } from '../resources/ScopedNavigation';
import { Button } from '../ui/Button';
import { sameAgent } from '../agent/status';
import { Tree, type TreeAction, type TreePermissions } from './Tree';
import type { NavigationState } from './navigation';
import { NavigationFilters, type NavigationView } from './NavigationFilters';

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
  readonly onViewDeletion: (id: string) => void;
  readonly onNavigate: (route: AppRoute) => void;
  readonly onAction: (action: TreeAction) => void;
}) {
  const [view, setView] = useState<NavigationView>('agent');
  const [statuses, setStatuses] = useState({ agent: 'all', sandbox: 'all' });
  const status = statuses[view];
  return (
    <>
      <NavigationFilters
        view={view}
        status={status}
        onView={setView}
        onStatus={(next) => setStatuses((previous) => ({ ...previous, [view]: next }))}
      />
      <ScopedNavigation
        route={route}
        client={client}
        scope={scope}
        choices={choices}
        view={view}
        statusFilter={status}
        onNavigate={onNavigate}
        personal={
          navigation.status === 'ready' ? (
            <Tree
              resourceClient={client}
              viewerId={viewerId}
              view={view}
              statusFilter={status}
              items={navigation.items.map((item) => ({
                ...item,
                workspaces: item.workspaces.map((workspace) => ({
                  ...workspace,
                  agents: workspace.agents.map((summary) =>
                    currentAgent && sameAgent(currentAgent, summary)
                      ? {
                          ...summary,
                          status: currentAgent.status,
                          title: currentAgent.title,
                          runtimeMetadata: currentAgent.runtimeMetadata,
                        }
                      : summary,
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
          ) : navigation.status === 'loading' ? (
            <p className="console__navigation-status" role="status">
              Loading Agents
            </p>
          ) : (
            <div className="console__navigation-status" role="alert">
              <p>{navigation.message}</p>
              <Button onClick={navigation.refresh}>Try again</Button>
            </div>
          )
        }
      />
    </>
  );
}
