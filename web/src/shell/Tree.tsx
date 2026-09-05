import { useState } from 'react';
import type { KeyboardEvent, ReactNode } from 'react';
import type { AgentStatus, SandboxStatus } from '../api/types';
import type { AppRoute } from '../routing/routes';
import { StateDot, type StateDotState } from '../ui/StateDot';
import {
  IconAgentPresetOutline16,
  IconBranchOutline16,
  IconDataOutline16,
  IconFolderOpenOutline16,
  IconPauseOutline16,
  IconPlayOutline16,
  IconPlusOutline16,
  IconStopFill16,
  IconTrashOutline16,
} from '../ui/icons';
import type { NavigationSandbox } from './navigation';
import { TreeRow, type TreeRowAction } from './TreeRow';
import './Tree.css';

export interface TreePermissions {
  readonly createWorkspace: boolean;
  readonly createAgent: boolean;
  readonly pauseSandbox: boolean;
  readonly deleteSandbox: boolean;
  readonly removeWorkspace: boolean;
  readonly stopAgent: boolean;
  readonly deleteAgent: boolean;
}

export type TreeAction =
  | { readonly kind: 'new_workspace'; readonly sandboxId: string }
  | { readonly kind: 'pause_sandbox'; readonly sandboxId: string }
  | { readonly kind: 'revive_sandbox'; readonly sandboxId: string }
  | { readonly kind: 'delete_sandbox'; readonly sandboxId: string }
  | { readonly kind: 'new_agent'; readonly workspaceId: string }
  | { readonly kind: 'remove_workspace'; readonly workspaceId: string }
  | { readonly kind: 'stop_agent'; readonly agentId: string }
  | { readonly kind: 'delete_agent'; readonly agentId: string };

interface TreeProps {
  readonly viewerId: string;
  readonly items: readonly NavigationSandbox[];
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

function statusLabel(status: string): string {
  return status.replaceAll('_', ' ').replace(/^./, (letter) => letter.toUpperCase());
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

function stateMeta(status: string, extra?: ReactNode) {
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

function sandboxActions(status: SandboxStatus, permissions: TreePermissions): TreeRowAction[] {
  const actions: TreeRowAction[] = [];
  if (permissions.createWorkspace)
    actions.push({ id: 'new_workspace', label: 'New workspace', icon: <IconPlusOutline16 /> });
  if (permissions.pauseSandbox) {
    actions.push(
      status === 'paused'
        ? { id: 'revive_sandbox', label: 'Revive', icon: <IconPlayOutline16 /> }
        : {
            id: 'pause_sandbox',
            label: 'Pause',
            icon: <IconPauseOutline16 />,
            disabled: status !== 'active',
          },
    );
  }
  if (permissions.deleteSandbox)
    actions.push({
      id: 'delete_sandbox',
      label: 'Delete',
      icon: <IconTrashOutline16 />,
      danger: true,
    });
  return actions;
}

function workspaceActions(permissions: TreePermissions): TreeRowAction[] {
  const actions: TreeRowAction[] = [];
  if (permissions.createAgent)
    actions.push({ id: 'new_agent', label: 'New agent', icon: <IconPlusOutline16 /> });
  if (permissions.removeWorkspace)
    actions.push({
      id: 'remove_workspace',
      label: 'Remove',
      icon: <IconTrashOutline16 />,
      danger: true,
    });
  return actions;
}

function agentActions(status: AgentStatus, permissions: TreePermissions): TreeRowAction[] {
  const actions: TreeRowAction[] = [];
  if (permissions.stopAgent)
    actions.push({
      id: 'stop_agent',
      label: 'Stop',
      icon: <IconStopFill16 />,
      disabled: !['starting', 'ready', 'working', 'waiting_approval', 'paused'].includes(status),
    });
  if (permissions.deleteAgent)
    actions.push({
      id: 'delete_agent',
      label: 'Delete',
      icon: <IconTrashOutline16 />,
      danger: true,
      disabled: !['stopped', 'failed'].includes(status),
    });
  return actions;
}

export function Tree({ viewerId, items, route, permissions, onNavigate, onAction }: TreeProps) {
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
    <div
      className="resource-tree"
      role="tree"
      aria-label="Sandboxes"
      onKeyDown={keyboardNavigation}
    >
      {items.map(({ sandbox, workspaces }) => {
        const sandboxNode = `sandbox:${sandbox.id}`;
        const sandboxExpanded = !collapsed.has(sandboxNode);
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
              expanded={sandboxExpanded}
              reviving={sandbox.status === 'reviving'}
              meta={stateMeta(
                sandbox.status,
                paused ? (
                  <span
                    className="tree-row__paused"
                    role="img"
                    aria-label="Sandbox paused"
                    title="Sandbox paused"
                  >
                    <IconPauseOutline16 />
                  </span>
                ) : undefined,
              )}
              actions={sandboxActions(sandbox.status, permissions)}
              onNavigate={onNavigate}
              onToggle={() => toggle(sandboxNode)}
              onAction={(kind) =>
                onAction({
                  kind: kind as
                    'new_workspace' | 'pause_sandbox' | 'revive_sandbox' | 'delete_sandbox',
                  sandboxId: sandbox.id,
                })
              }
            />
            {sandboxExpanded ? (
              <div role="group">
                {workspaces.map((workspace) => {
                  const workspaceNode = `workspace:${workspace.id}`;
                  const workspaceExpanded = !collapsed.has(workspaceNode);
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
                        meta={statusLabel(workspace.status)}
                        expanded={workspaceExpanded}
                        actions={workspaceActions(permissions)}
                        onNavigate={onNavigate}
                        onToggle={() => toggle(workspaceNode)}
                        onAction={(kind) =>
                          onAction({
                            kind: kind as 'new_agent' | 'remove_workspace',
                            workspaceId: workspace.id,
                          })
                        }
                      />
                      {workspaceExpanded ? (
                        <div role="group">
                          {workspace.agents.map((agent) => {
                            const effectiveStatus = paused ? 'paused' : agent.status;
                            return (
                              <TreeRow
                                key={agent.id}
                                nodeId={`agent:${agent.id}`}
                                parentId={workspaceNode}
                                level={3}
                                label={agent.title?.trim() || 'Untitled agent'}
                                icon={<IconAgentPresetOutline16 />}
                                route={{ kind: 'agent', agentId: agent.id }}
                                selected={route.kind === 'agent' && route.agentId === agent.id}
                                dimmed={paused}
                                meta={stateMeta(
                                  effectiveStatus,
                                  <time dateTime={agent.updatedAt}>
                                    {activityLabel(agent.updatedAt)}
                                  </time>,
                                )}
                                actions={agentActions(agent.status, permissions)}
                                onNavigate={onNavigate}
                                onAction={(kind) =>
                                  onAction({
                                    kind: kind as 'stop_agent' | 'delete_agent',
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
      {items.length === 0 ? <p className="resource-tree__empty">No Sandboxes</p> : null}
    </div>
  );
}
