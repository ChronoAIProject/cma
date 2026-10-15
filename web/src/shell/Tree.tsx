import { Presence } from '../ui/Presence';
import { useId, useState } from 'react';
import {
  sandboxChildrenAvailable,
  sandboxChildrenReadable,
  sandboxChildStatus,
  sandboxLifecycleLabel,
} from '../sandbox/lifecycle';
import type { AppRoute } from '../routing/routes';
import { StateDot } from '../ui/StateDot';
import { Button } from '../ui/Button';
import {
  IconAgentPresetOutline16,
  IconBranchOutline16,
  IconChevronDownOutline14,
  IconChevronRightOutline14,
  IconDataOutline16,
  IconFolderOpenOutline16,
  IconPauseOutline16,
} from '../ui/icons';
import type { NavigationSandbox } from './navigation';
import { TreeRow } from './TreeRow';
import { agentActions, sandboxActions, workspaceActions } from './treeActions';
import { CreationBadge, useTreeCreation } from './treeCreation';
import './Tree.css';
import { statusLabel } from '../vocabulary/status';
import { dotFor, stateMeta, keyboardNavigation } from './treePresentation';
import type { ResourceViewsClient } from '../resources/client';
import { useChannelFacts } from '../resources/useChannelFacts';
import { ChannelBadge, TreeFactsSummary, treeAgentIds } from './treeChannel';
import { agentDisplayTitle } from '../agent/presentation';

export interface TreePermissions {
  readonly createWorkspace: boolean;
  readonly createAgent: boolean;
  readonly pauseSandbox: boolean;
  readonly deleteSandbox: boolean;
  readonly removeWorkspace: boolean;
  readonly stopAgent: boolean;
  readonly deleteAgent: boolean;
  readonly exportHistory: boolean;
}

export type TreeAction =
  | { readonly kind: 'new_workspace'; readonly sandboxId: string }
  | { readonly kind: 'pause_sandbox'; readonly sandboxId: string }
  | { readonly kind: 'revive_sandbox'; readonly sandboxId: string }
  | { readonly kind: 'delete_sandbox'; readonly sandboxId: string }
  | { readonly kind: 'new_agent'; readonly workspaceId: string }
  | { readonly kind: 'new_agent'; readonly sandboxId: string }
  | { readonly kind: 'remove_workspace'; readonly workspaceId: string }
  | { readonly kind: 'stop_agent'; readonly agentId: string }
  | { readonly kind: 'delete_agent'; readonly agentId: string }
  | { readonly kind: 'export_history'; readonly agentId: string };

interface TreeProps {
  readonly resourceClient?: ResourceViewsClient;
  readonly viewerId: string;
  readonly items: readonly NavigationSandbox[];
  readonly refreshError?: string;
  readonly lifecycleBusy?: boolean;
  readonly busySandboxIds?: ReadonlySet<string>;
  readonly deletingSandboxIds?: ReadonlySet<string>;
  readonly onViewDeletion?: (sandboxId: string) => void;
  readonly route: AppRoute;
  readonly permissions: TreePermissions;
  readonly onNavigate: (route: AppRoute) => void;
  readonly onAction: (action: TreeAction) => void;
}

function storageKey(viewerId: string): string {
  return `cma:tree:${viewerId}:collapsed`;
}

function restored(viewerId: string): Set<string> {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(storageKey(viewerId)) ?? '[]');
    return new Set(
      Array.isArray(parsed)
        ? parsed.filter((value): value is string => typeof value === 'string')
        : [],
    );
  } catch {
    return new Set();
  }
}

export function activityLabel(value: string, now = Date.now()): string {
  const elapsed = Math.max(0, now - Date.parse(value));
  if (!Number.isFinite(elapsed)) return 'Unknown activity';
  if (elapsed < 60_000) return 'Now';
  if (elapsed < 3_600_000) return `${String(Math.floor(elapsed / 60_000))}m`;
  if (elapsed < 86_400_000) return `${String(Math.floor(elapsed / 3_600_000))}h`;
  return `${String(Math.floor(elapsed / 86_400_000))}d`;
}

export function Tree({
  resourceClient,
  viewerId,
  items,
  refreshError,
  lifecycleBusy = false,
  busySandboxIds,
  deletingSandboxIds,
  onViewDeletion,
  route,
  permissions,
  onNavigate,
  onAction,
}: TreeProps) {
  const deletingItems = items.filter(
    ({ sandbox }) =>
      sandbox.status !== 'retired' &&
      (sandbox.status === 'retiring' || deletingSandboxIds?.has(sandbox.id)),
  );
  const visibleItems = items.filter(
    (item) => item.sandbox.status !== 'retired' && !deletingItems.includes(item),
  );
  const facts = useTreeCreation(viewerId, visibleItems, refreshError === undefined);
  const channelIds = treeAgentIds(visibleItems);
  const channelFacts = useChannelFacts(
    resourceClient,
    channelIds,
    refreshError === undefined,
    viewerId,
  );
  const groupId = useId();
  const groups = [
    {
      id: 'live',
      label: 'Live',
      items: visibleItems.filter(({ sandbox }) => sandboxLifecycleLabel(sandbox) !== 'Paused'),
    },
    {
      id: 'paused',
      label: 'Paused',
      items: visibleItems.filter(({ sandbox }) => sandboxLifecycleLabel(sandbox) === 'Paused'),
    },
  ];
  const [collapsed, setCollapsed] = useState(() => restored(viewerId));
  const toggle = (node: string) => {
    setCollapsed((current) => {
      const next = new Set(current);
      if (next.has(node)) next.delete(node);
      else next.add(node);
      localStorage.setItem(storageKey(viewerId), JSON.stringify([...next]));
      return next;
    });
  };

  return (
    <div className="resource-tree">
      {deletingItems.length === 0 ? null : (
        <details className="resource-tree__notice">
          <summary>Deletion progress ({deletingItems.length})</summary>
          {deletingItems.map(({ sandbox }) => (
            <Button
              key={sandbox.id}
              size="sm"
              aria-label={`Deletion status for ${sandbox.name}`}
              onClick={() =>
                onViewDeletion
                  ? onViewDeletion(sandbox.id)
                  : onNavigate({ kind: 'sandbox', sandboxId: sandbox.id })
              }
            >
              {sandbox.name}
            </Button>
          ))}
        </details>
      )}
      <Presence>
        {refreshError === undefined ? null : (
          <p className="resource-tree__notice" role="alert">
            {refreshError}
          </p>
        )}
      </Presence>
      {groups.map((group) => {
        const groupNode = `group:${group.id}`;
        const expanded = !collapsed.has(groupNode);
        return (
          <section
            className="resource-tree__section"
            key={group.id}
            aria-label={`${group.label} sandboxes`}
          >
            <button
              type="button"
              className="resource-tree__heading"
              aria-label={`${expanded ? 'Collapse' : 'Expand'} ${group.label} sandboxes`}
              aria-expanded={expanded}
              aria-controls={`${groupId}-${group.id}`}
              onClick={() => toggle(groupNode)}
            >
              {expanded ? <IconChevronDownOutline14 /> : <IconChevronRightOutline14 />}
              <span>{group.label}</span>
              <span className="resource-tree__count">{group.items.length}</span>
            </button>
            <div id={`${groupId}-${group.id}`} hidden={!expanded}>
              {expanded ? (
                <div
                  role="tree"
                  aria-label={`${group.label} sandboxes`}
                  onKeyDown={keyboardNavigation}
                >
                  {group.items.map(({ sandbox, workspaces }) => {
                    const sandboxBusy = lifecycleBusy || busySandboxIds?.has(sandbox.id) === true;
                    const originalWorkspaces =
                      items.find((item) => item.sandbox.id === sandbox.id)?.workspaces ?? [];
                    const sandboxNode = `sandbox:${sandbox.id}`;
                    const childrenAvailable = sandboxChildrenReadable(sandbox);
                    const runtimeAvailable =
                      !sandboxBusy && !refreshError && sandboxChildrenAvailable(sandbox);
                    const sandboxExpanded = childrenAvailable && !collapsed.has(sandboxNode);
                    const paused = sandbox.status === 'paused';
                    return (
                      <div className="resource-tree__branch" key={sandbox.id}>
                        <TreeRow
                          nodeId={sandboxNode}
                          level={1}
                          label={`Sandbox Machine - ${sandbox.id.replace(/^sbx_/, '')}`}
                          icon={<IconDataOutline16 />}
                          route={{ kind: 'sandbox', sandboxId: sandbox.id }}
                          selected={route.kind === 'sandbox' && route.sandboxId === sandbox.id}
                          expanded={childrenAvailable ? sandboxExpanded : undefined}
                          reviving={sandbox.status === 'reviving'}
                          meta={
                            <>
                              <span className="tree-row__state">
                                <StateDot
                                  state={
                                    refreshError !== undefined
                                      ? 'warning'
                                      : sandbox.lifecycle?.operation !== undefined
                                        ? 'ongoing'
                                        : dotFor(sandbox.status)
                                  }
                                  size={8}
                                />
                                {sandboxLifecycleLabel(sandbox)}
                              </span>
                              <TreeFactsSummary
                                channelFacts={channelFacts}
                                facts={facts}
                                ids={originalWorkspaces
                                  .filter((workspace) => workspace.status !== 'removed')
                                  .flatMap((workspace) =>
                                    workspace.agents
                                      .filter((agent) => agent.status !== 'deleted')
                                      .map((agent) => agent.id),
                                  )}
                              />
                              {paused ? (
                                <span
                                  className="tree-row__paused"
                                  role="img"
                                  aria-label="Sandbox paused"
                                  title="Sandbox paused"
                                >
                                  <IconPauseOutline16 />
                                </span>
                              ) : undefined}
                            </>
                          }
                          actions={sandboxActions(
                            sandbox,
                            permissions,
                            refreshError !== undefined,
                            sandboxBusy,
                          )}
                          onNavigate={onNavigate}
                          onToggle={childrenAvailable ? () => toggle(sandboxNode) : undefined}
                          onAction={(kind) =>
                            kind === 'new_agent'
                              ? onAction({ kind, sandboxId: sandbox.id })
                              : onAction({
                                  kind: kind as
                                    | 'new_workspace'
                                    | 'pause_sandbox'
                                    | 'revive_sandbox'
                                    | 'delete_sandbox',
                                  sandboxId: sandbox.id,
                                })
                          }
                        />
                        {sandboxExpanded ? (
                          <div role="group">
                            {workspaces
                              .filter((workspace) => workspace.status !== 'removed')
                              .map((workspace) => {
                                const workspaceNode = `workspace:${workspace.id}`;
                                const workspaceReady = workspace.status === 'ready';
                                const selectedAgent =
                                  route.kind === 'agent'
                                    ? workspace.agents.find((agent) => agent.id === route.agentId)
                                    : undefined;
                                const workspaceExpanded =
                                  workspaceReady && !collapsed.has(workspaceNode);
                                return (
                                  <div className="resource-tree__branch" key={workspace.id}>
                                    <TreeRow
                                      nodeId={workspaceNode}
                                      parentId={sandboxNode}
                                      level={2}
                                      label={
                                        selectedAgent?.runtimeMetadata?.workingDirectory?.trim() ||
                                        workspace.path ||
                                        workspace.name
                                      }
                                      icon={
                                        workspace.source.type === 'git_repository' ? (
                                          <IconBranchOutline16 />
                                        ) : (
                                          <IconFolderOpenOutline16 />
                                        )
                                      }
                                      meta={
                                        <>
                                          {statusLabel(
                                            sandboxChildStatus(sandbox, workspace.status),
                                          )}
                                          <TreeFactsSummary
                                            channelFacts={channelFacts}
                                            facts={facts}
                                            ids={(
                                              originalWorkspaces.find(
                                                (value) => value.id === workspace.id,
                                              )?.agents ?? []
                                            )
                                              .filter((agent) => agent.status !== 'deleted')
                                              .map((agent) => agent.id)}
                                          />
                                        </>
                                      }
                                      expanded={workspaceReady ? workspaceExpanded : undefined}
                                      actions={workspaceActions(
                                        workspace,
                                        permissions,
                                        sandboxBusy,
                                        runtimeAvailable,
                                      )}
                                      onNavigate={onNavigate}
                                      onToggle={
                                        workspaceReady ? () => toggle(workspaceNode) : undefined
                                      }
                                      onAction={(kind) =>
                                        kind === 'new_agent'
                                          ? onAction({ kind, sandboxId: sandbox.id })
                                          : onAction({
                                              kind: 'remove_workspace',
                                              workspaceId: workspace.id,
                                            })
                                      }
                                    />
                                    {workspaceExpanded ? (
                                      <div role="group">
                                        {workspace.agents
                                          .filter((agent) => agent.status !== 'deleted')
                                          .map((agent) => {
                                            const effectiveStatus = sandboxChildStatus(
                                              sandbox,
                                              agent.status,
                                            );
                                            return (
                                              <TreeRow
                                                key={agent.id}
                                                nodeId={`agent:${agent.id}`}
                                                parentId={workspaceNode}
                                                level={3}
                                                label={agentDisplayTitle(agent)}
                                                icon={<IconAgentPresetOutline16 />}
                                                route={{ kind: 'agent', agentId: agent.id }}
                                                selected={
                                                  route.kind === 'agent' &&
                                                  route.agentId === agent.id
                                                }
                                                dimmed={paused}
                                                meta={stateMeta(
                                                  effectiveStatus,
                                                  <>
                                                    <CreationBadge id={agent.id} facts={facts} />
                                                    <ChannelBadge
                                                      id={agent.id}
                                                      facts={channelFacts}
                                                    />
                                                    <time dateTime={agent.updatedAt}>
                                                      {activityLabel(agent.updatedAt)}
                                                    </time>
                                                  </>,
                                                )}
                                                actions={agentActions(
                                                  agent.status,
                                                  permissions,
                                                  runtimeAvailable,
                                                )}
                                                onNavigate={onNavigate}
                                                onAction={(kind) =>
                                                  onAction({
                                                    kind: kind as
                                                      | 'stop_agent'
                                                      | 'delete_agent'
                                                      | 'export_history',
                                                    agentId: agent.id,
                                                  })
                                                }
                                              />
                                            );
                                          })}
                                      </div>
                                    ) : null}
                                  </div>
                                );
                              })}
                          </div>
                        ) : null}
                      </div>
                    );
                  })}
                  {group.items.length === 0 ? (
                    <p className="resource-tree__empty">No {group.label.toLowerCase()} sandboxes</p>
                  ) : null}
                </div>
              ) : null}
            </div>
          </section>
        );
      })}
      {visibleItems.length === 0 ? <p className="resource-tree__empty">No Sandboxes</p> : null}
    </div>
  );
}
