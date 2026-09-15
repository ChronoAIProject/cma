import { useMemo, useState } from 'react';
import { ApiError } from '@chronoai/cma-sdk';
import type { ResourceViewsClient } from './client';
import type { AppRoute } from '../routing/routes';
import { useAuthorizedRead } from './useAuthorizedRead';
import { loadWorkspace } from './loadHierarchy';
import { ResourceLink } from './ResourceLink';
import { statusLabel } from '../vocabulary/status';
import { resourceCount } from '../vocabulary/count';
import { IconChevronDownOutline14, IconChevronRightOutline14 } from '../ui/icons';
import { Button } from '../ui/Button';
import { useChannelFacts } from './useChannelFacts';
import { channelLabel } from './channelLabels';

export function WorkspaceBranch({
  client,
  id,
  sandboxId,
  name,
  onNavigate,
  onAccessChanged,
}: {
  readonly client: ResourceViewsClient;
  readonly id: string;
  readonly sandboxId: string;
  readonly name: string;
  readonly onNavigate: (route: AppRoute) => void;
  readonly onAccessChanged: () => void;
}) {
  const [open, setOpen] = useState(false);
  return (
    <li className="shared-hierarchy__workspace">
      <Button
        size="sm"
        icon={open ? <IconChevronDownOutline14 /> : <IconChevronRightOutline14 />}
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
      >
        {name}
      </Button>
      {open ? (
        <WorkspaceContents
          client={client}
          id={id}
          sandboxId={sandboxId}
          onNavigate={onNavigate}
          onAccessChanged={onAccessChanged}
        />
      ) : null}
    </li>
  );
}

export function WorkspaceContents({
  client,
  id,
  sandboxId,
  onNavigate,
  onAccessChanged,
}: {
  readonly client: ResourceViewsClient;
  readonly id: string;
  readonly sandboxId?: string;
  readonly onNavigate: (route: AppRoute) => void;
  readonly onAccessChanged?: () => void;
}) {
  const read = useMemo(
    () => async (signal: AbortSignal) => {
      try {
        return await loadWorkspace(client, id, signal, sandboxId);
      } catch (error) {
        if (
          !signal.aborted &&
          error instanceof ApiError &&
          [401, 403, 404, 409].includes(error.status)
        )
          onAccessChanged?.();
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
  if (current.state.status === 'loading') return <p role="status">Loading authorized Workspace</p>;
  if (current.state.status === 'error')
    return (
      <div role="alert">
        <p>{current.state.message}</p>
        <Button onClick={current.reload}>Check Workspace access</Button>
      </div>
    );
  const workspace = current.state.value.detail;
  return (
    <div>
      {sandboxId === undefined ? <h2>{workspace.name}</h2> : null}
      <p>
        {statusLabel(workspace.status)} · {resourceCount(workspace.worktree_count, 'Worktree')} ·{' '}
        {resourceCount(workspace.agent_count, 'Agent')}
      </p>
      {sandboxId === undefined ? null : (
        <ResourceLink route={{ kind: 'workspace', workspaceId: id }} onNavigate={onNavigate}>
          Open Workspace details
        </ResourceLink>
      )}
      <ul className="shared-hierarchy__agents">
        {workspace.agents.map((agent) => (
          <li key={agent.id}>
            <ResourceLink route={{ kind: 'agent', agentId: agent.id }} onNavigate={onNavigate}>
              {agent.title?.trim() || 'Untitled agent'}
            </ResourceLink>
            <span>{statusLabel(agent.status)}</span>
            <span>{channelLabel(channelFacts.get(agent.id))}</span>
          </li>
        ))}
      </ul>
      {workspace.agents.length === 0 ? <p>No authorized Agents in this Workspace.</p> : null}
      <ul className="shared-hierarchy__worktrees" aria-label="Workspace Worktrees">
        {workspace.worktrees.map((worktree) => (
          <li key={worktree.id}>
            <span>{worktree.name}</span>
            <span>{statusLabel(worktree.status)}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}
