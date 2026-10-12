import { Presence } from '../ui/Presence';
import { useId, useState } from 'react';
import type { KeyboardEvent, ReactNode } from 'react';
import type { AgentStatus, SandboxStatus } from '../api/types';
import { sandboxChildrenAvailable, sandboxLifecycleLabel } from '../sandbox/lifecycle';
import type { AppRoute } from '../routing/routes';
import { StateDot, type StateDotState } from '../ui/StateDot';
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
import {
  CreationBadge,
  filterTreeCreation,
  TreeCreationFilter,
  useTreeCreation,
  type CreationFilter,
} from './treeCreation';
import './Tree.css';
import { statusLabel } from '../vocabulary/status';
import type { ResourceViewsClient } from '../resources/client';
import { useChannelFacts } from '../resources/useChannelFacts';
import type { ChannelFilter } from '../resources/channelLabels';
import {
  ChannelBadge,
  TreeFactsSummary,
  TreeChannelFilter,
  filterTreeChannel,
  treeAgentIds,
} from './treeChannel';

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

function dotFor(status: string): StateDotState {
  if (status === 'failed' || status === 'deleted' || status === 'retired') return 'error';
  if (status === 'paused' || status === 'waiting_approval') return 'warning';
  if (
    [
      'working',
      'starting',
      'provisioning',
      'installing',
      'pausing',
      'reviving',
      'rotating',
      'credential_rotating',
      'retiring',
      'removing',
    ].includes(status)
  )
    return 'ongoing';
  return 'done';
}

function stateMeta(status: SandboxStatus | AgentStatus, extra?: ReactNode) {
  return (
    <>
      <span className="tree-row__state">
        <StateDot state={dotFor(status)} size={8} />
        {statusLabel(status)}
      </span>
      {extra}
    </>
  );
}

export function activityLabel(value: string, now = Date.now()): string {
  const elapsed = Math.max(0, now - Date.parse(value));
  if (!Number.isFinite(elapsed)) return 'Unknown activity';
  if (elapsed < 60_000) return 'Now';
  if (elapsed < 3_600_000) return `${String(Math.floor(elapsed / 60_000))}m`;
  if (elapsed < 86_400_000) return `${String(Math.floor(elapsed / 3_600_000))}h`;
  return `${String(Math.floor(elapsed / 86_400_000))}d`;
}

function visibleRows(tree: HTMLElement): HTMLElement[] {
  return [...tree.querySelectorAll<HTMLElement>('[role="treeitem"]')];
}

function keyboardNavigation(event: KeyboardEvent<HTMLDivElement>) {
  if (!(event.target instanceof HTMLElement) || event.target.getAttribute('role') !== 'treeitem')
    return;
  const row = event.target;
  const tree = event.currentTarget;
  const rows = visibleRows(tree);
  const at = rows.indexOf(row);
  const move = (index: number) => {
    event.preventDefault();
    rows[index]?.focus();
  };
  if (event.key === 'ArrowDown') move(at + 1);
  else if (event.key === 'ArrowUp') move(at - 1);
  else if (event.key === 'Home') move(0);
  else if (event.key === 'End') move(rows.length - 1);
  else if (event.key === 'ArrowRight') {
    const disclosure = row.querySelector<HTMLButtonElement>('.tree-row__disclosure');
    if (row.getAttribute('aria-expanded') === 'false') disclosure?.click();
    else rows.find((candidate) => candidate.dataset.parentId === row.dataset.nodeId)?.focus();
    event.preventDefault();
  } else if (event.key === 'ArrowLeft') {
    if (row.getAttribute('aria-expanded') === 'true') {
      row.querySelector<HTMLButtonElement>('.tree-row__disclosure')?.click();
    } else {
      rows.find((candidate) => candidate.dataset.nodeId === row.dataset.parentId)?.focus();
    }
    event.preventDefault();
  } else if (event.key === 'Enter' || event.key === ' ') {
    row.querySelector<HTMLElement>('.tree-row__label')?.click();
    event.preventDefault();
  }
}

export function Tree({
  resourceClient,
  viewerId,
  items,
  refreshError,
  lifecycleBusy = false,
  route,
  permissions,
  onNavigate,
  onAction,
}: TreeProps) {
  const facts = useTreeCreation(viewerId, items, refreshError === undefined);
  const channelIds = treeAgentIds(items);
  const channelFacts = useChannelFacts(
    resourceClient,
    channelIds,
    refreshError === undefined,
    viewerId,
  );
  const [channelSelection, setChannelSelection] = useState({
    viewerId,
    value: 'all' as ChannelFilter,
  });
  const channelFilter = channelSelection.viewerId === viewerId ? channelSelection.value : 'all';
  const [selection, setSelection] = useState({ viewerId, value: 'all' as CreationFilter });
  const filter = selection.viewerId === viewerId ? selection.value : 'all';
  const visibleItems = filterTreeChannel(
    filterTreeCreation(items, facts, filter),
    channelFacts,
    channelFilter,
  ).filter(({ sandbox }) => sandbox.status !== 'retired');
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
      <TreeCreationFilter
        items={items}
        facts={facts}
        value={filter}
        onChange={(value) => setSelection({ viewerId, value })}
      />
      {resourceClient === undefined ? null : (
        <TreeChannelFilter
          ids={channelIds}
          facts={channelFacts}
          value={channelFilter}
          onChange={(value) => setChannelSelection({ viewerId, value })}
        />
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
                    const originalWorkspaces =
                      items.find((item) => item.sandbox.id === sandbox.id)?.workspaces ?? [];
                    const sandboxNode = `sandbox:${sandbox.id}`;
                    const childrenAvailable = sandboxChildrenAvailable(sandbox);
                    const sandboxExpanded = childrenAvailable && !collapsed.has(sandboxNode);
                    const paused = sandbox.status === 'paused';
                    return (
                      <div className="resource-tree__branch" key={sandbox.id}>
                        <TreeRow
                          nodeId={sandboxNode}
                          level={1}
                          label={sandbox.name}
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
                            lifecycleBusy,
                          )}
                          onNavigate={onNavigate}
                          onToggle={childrenAvailable ? () => toggle(sandboxNode) : undefined}
                          onAction={(kind) =>
                            onAction({
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
                                const workspaceExpanded =
                                  workspaceReady && !collapsed.has(workspaceNode);
                                return (
                                  <div className="resource-tree__branch" key={workspace.id}>
                                    <TreeRow
                                      nodeId={workspaceNode}
                                      parentId={sandboxNode}
                                      level={2}
                                      label={workspace.name}
                                      icon={
                                        workspace.source.type === 'git_repository' ? (
                                          <IconBranchOutline16 />
                                        ) : (
                                          <IconFolderOpenOutline16 />
                                        )
                                      }
                                      meta={
                                        <>
                                          {workspace.status === 'removing'
                                            ? 'Deleting'
                                            : statusLabel(workspace.status)}
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
                                        lifecycleBusy,
                                      )}
                                      onNavigate={onNavigate}
                                      onToggle={
                                        workspaceReady ? () => toggle(workspaceNode) : undefined
                                      }
                                      onAction={(kind) =>
                                        onAction({
                                          kind: kind as 'new_agent' | 'remove_workspace',
                                          workspaceId: workspace.id,
                                        })
                                      }
                                    />
                                    {workspaceExpanded ? (
                                      <div role="group">
                                        {workspace.agents
                                          .filter((agent) => agent.status !== 'deleted')
                                          .map((agent) => {
                                            const effectiveStatus = paused
                                              ? 'paused'
                                              : agent.status;
                                            return (
                                              <TreeRow
                                                key={agent.id}
                                                nodeId={`agent:${agent.id}`}
                                                parentId={workspaceNode}
                                                level={3}
                                                label={agent.title?.trim() || 'Untitled agent'}
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
                                                actions={agentActions(agent.status, permissions)}
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
      {visibleItems.length === 0 ? (
        <p className="resource-tree__empty">
          {filter === 'all' && channelFilter === 'all'
            ? 'No Sandboxes'
            : 'No matching Agents in this tree'}
        </p>
      ) : null}
    </div>
  );
}
