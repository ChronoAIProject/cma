import { useCallback, useMemo } from 'react';
import { ApiError, type AccessibleSandboxSummary } from '@chronoai/cma-sdk';
import type { AppRoute } from '../routing/routes';
import { TreeRow } from '../shell/TreeRow';
import { stateMeta } from '../shell/treePresentation';
import { IconAgentPresetOutline16, IconDataOutline16, IconFolderOpenOutline16 } from '../ui/icons';
import { resourceCount } from '../vocabulary/count';
import type { ResourceViewsClient } from './client';
import type { AccessSummary } from './accessSummary';
import { ResourceAccessSummary } from './ResourceAccessSummary';
import { loadSandbox, loadWorkspace } from './loadHierarchy';
import { useAuthorizedRead } from './useAuthorizedRead';
import { useChannelFacts } from './useChannelFacts';
import { channelLabel } from './channelLabels';

export interface TreeExpansion {
  readonly collapsed: ReadonlySet<string>;
  readonly toggle: (id: string) => void;
  readonly collapse: (id: string) => void;
}

interface BranchProps {
  readonly expansion: TreeExpansion;
  readonly client: ResourceViewsClient;
  readonly route: AppRoute;
  readonly onNavigate: (route: AppRoute) => void;
  readonly onAccessChanged: () => void;
}

function denied(error: unknown, signal: AbortSignal) {
  return (
    !signal.aborted && error instanceof ApiError && [401, 403, 404, 409].includes(error.status)
  );
}

/** Same rows as Personal, populated only by the authorized hierarchy projection. */
export function SandboxBranch({
  client,
  expansion,
  sandbox,
  accessSummary,
  route,
  onNavigate,
  onAccessChanged,
}: BranchProps & {
  readonly sandbox: AccessibleSandboxSummary;
  readonly accessSummary?: AccessSummary;
}) {
  const node = `sandbox:${sandbox.id}`;
  const open = !expansion.collapsed.has(node);
  const collapse = expansion.collapse;
  const accessChanged = useCallback(() => {
    collapse(node);
    onAccessChanged();
  }, [collapse, node, onAccessChanged]);
  return (
    <div className="resource-tree__branch">
      <TreeRow
        nodeId={`sandbox:${sandbox.id}`}
        level={1}
        label={sandbox.name}
        icon={<IconDataOutline16 />}
        route={{ kind: 'sandbox', sandboxId: sandbox.id }}
        selected={route.kind === 'sandbox' && route.sandboxId === sandbox.id}
        expanded={open}
        reviving={sandbox.status === 'reviving'}
        meta={stateMeta(
          sandbox.status,
          <>
            <span>{resourceCount(sandbox.agent_count, 'Agent')}</span>
            <ResourceAccessSummary compact value={accessSummary} />
          </>,
          'sandbox',
        )}
        onNavigate={onNavigate}
        onToggle={() => expansion.toggle(node)}
      />
      {open ? (
        <SandboxChildren
          client={client}
          id={sandbox.id}
          route={route}
          onNavigate={onNavigate}
          onAccessChanged={accessChanged}
          expansion={expansion}
        />
      ) : null}
    </div>
  );
}

function SandboxChildren({
  client,
  expansion,
  id,
  route,
  onNavigate,
  onAccessChanged,
}: BranchProps & {
  readonly id: string;
}) {
  const read = useMemo(
    () => async (signal: AbortSignal) => {
      try {
        return await loadSandbox(client, id, signal);
      } catch (error) {
        if (denied(error, signal)) onAccessChanged();
        throw error;
      }
    },
    [client, id, onAccessChanged],
  );
  const current = useAuthorizedRead(read);
  if (current.state.status === 'loading')
    return (
      <p className="resource-tree__notice" role="status">
        Loading Workspaces
      </p>
    );
  if (current.state.status === 'error')
    return (
      <p className="resource-tree__notice" role="alert">
        {current.state.message}
      </p>
    );
  return (
    <div role="group">
      {current.state.value.detail.workspaces.map((workspace) => (
        <WorkspaceTreeBranch
          key={workspace.id}
          client={client}
          id={workspace.id}
          sandboxId={id}
          name={workspace.name}
          status={workspace.status}
          route={route}
          onNavigate={onNavigate}
          onAccessChanged={current.reload}
          expansion={expansion}
        />
      ))}
      {current.state.value.detail.workspaces.length === 0 ? (
        <p className="resource-tree__empty">No authorized Workspaces</p>
      ) : null}
    </div>
  );
}

function WorkspaceTreeBranch({
  client,
  expansion,
  id,
  sandboxId,
  name,
  status,
  route,
  onNavigate,
  onAccessChanged,
}: BranchProps & {
  readonly id: string;
  readonly sandboxId: string;
  readonly name: string;
  readonly status: Parameters<typeof stateMeta>[0];
}) {
  const node = `workspace:${id}`;
  const open = !expansion.collapsed.has(node);
  const collapse = expansion.collapse;
  const accessChanged = useCallback(() => {
    collapse(node);
    onAccessChanged();
  }, [collapse, node, onAccessChanged]);
  return (
    <div className="resource-tree__branch">
      <TreeRow
        nodeId={`workspace:${id}`}
        parentId={`sandbox:${sandboxId}`}
        level={2}
        label={name}
        icon={<IconFolderOpenOutline16 />}
        route={{ kind: 'workspace', workspaceId: id }}
        selected={route.kind === 'workspace' && route.workspaceId === id}
        expanded={open}
        meta={stateMeta(status, undefined, 'workspace')}
        onNavigate={onNavigate}
        onToggle={() => expansion.toggle(node)}
      />
      {open ? (
        <WorkspaceChildren
          client={client}
          id={id}
          sandboxId={sandboxId}
          route={route}
          onNavigate={onNavigate}
          onAccessChanged={accessChanged}
        />
      ) : null}
    </div>
  );
}

function WorkspaceChildren({
  client,
  id,
  sandboxId,
  route,
  onNavigate,
  onAccessChanged,
}: Omit<BranchProps, 'expansion'> & {
  readonly id: string;
  readonly sandboxId: string;
}) {
  const read = useMemo(
    () => async (signal: AbortSignal) => {
      try {
        return await loadWorkspace(client, id, signal, sandboxId);
      } catch (error) {
        if (denied(error, signal)) onAccessChanged();
        throw error;
      }
    },
    [client, id, sandboxId, onAccessChanged],
  );
  const current = useAuthorizedRead(read);
  const channelFacts = useChannelFacts(
    client,
    current.state.status === 'ready'
      ? current.state.value.detail.agents.map((agent) => agent.id)
      : [],
    current.state.status === 'ready',
  );
  if (current.state.status === 'loading')
    return (
      <p className="resource-tree__notice" role="status">
        Loading Agents
      </p>
    );
  if (current.state.status === 'error')
    return (
      <p className="resource-tree__notice" role="alert">
        {current.state.message}
      </p>
    );
  return (
    <div role="group">
      {current.state.value.detail.agents
        .filter((agent) => agent.status !== 'deleted')
        .map((agent) => (
          <TreeRow
            key={agent.id}
            nodeId={`agent:${agent.id}`}
            parentId={`workspace:${id}`}
            level={3}
            label={agent.title?.trim() || 'Untitled agent'}
            icon={<IconAgentPresetOutline16 />}
            route={{ kind: 'agent', agentId: agent.id }}
            selected={route.kind === 'agent' && route.agentId === agent.id}
            meta={stateMeta(agent.status, <span>{channelLabel(channelFacts.get(agent.id))}</span>)}
            onNavigate={onNavigate}
          />
        ))}
      {current.state.value.detail.agents.length === 0 ? (
        <p className="resource-tree__empty">No authorized Agents</p>
      ) : null}
    </div>
  );
}
