import { useAccessSummaries } from './useAccessSummaries';
import { SkeletonRows } from '../ui/Skeleton';
import { ResourceAccessSummary, ResourceAccessDetails } from './ResourceAccessSummary';
import { useMemo, useState } from 'react';
import { ApiError } from '@chronoai/cma-sdk';
import type { ResourceViewsClient } from './client';
import { routePath, type AppRoute } from '../routing/routes';
import { PageHeader } from '../ui/PageHeader';
import { StatusBadge } from '../ui/Badge';
import { useDocumentTitle } from '../ui/useDocumentTitle';
import { useAuthorizedRead } from './useAuthorizedRead';
import { loadWorkspace } from './loadHierarchy';
import { ResourceLink } from './ResourceLink';
import { statusLabel } from '../vocabulary/status';
import { resourceCount } from '../vocabulary/count';
import { IconChevronDownOutline14, IconChevronRightOutline14 } from '../ui/icons';
import '../sandbox/SandboxPage.css';
import './SharedHierarchy.css';
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
    <li className="sandbox-page__workspace">
      <div className="sandbox-page__header">
        <h3>
          <ResourceLink route={{ kind: 'workspace', workspaceId: id }} onNavigate={onNavigate}>
            {name}
          </ResourceLink>
        </h3>
        <Button
          size="sm"
          icon={open ? <IconChevronDownOutline14 /> : <IconChevronRightOutline14 />}
          aria-label={`${open ? 'Hide' : 'Show'} Agents in ${name}`}
          aria-expanded={open}
          onClick={() => setOpen((value) => !value)}
        >
          {open ? 'Hide Agents' : 'Show Agents'}
        </Button>
      </div>
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
  const accessSummaries = useAccessSummaries(
    client,
    'agent',
    current.state.status === 'ready'
      ? current.state.value.detail.agents.map((agent) => agent.id)
      : [],
    id,
  );
  const channelFacts = useChannelFacts(
    client,
    current.state.status === 'ready'
      ? current.state.value.detail.agents.map((agent) => agent.id)
      : [],
    current.state.status === 'ready',
  );
  useDocumentTitle(
    sandboxId === undefined && current.state.status === 'ready'
      ? current.state.value.detail.name
      : undefined,
  );
  if (current.state.status === 'loading')
    return <SkeletonRows rows={2} label="Loading authorized Workspace" />;
  if (current.state.status === 'error')
    return (
      <div role="alert">
        <p>{current.state.message}</p>
        <Button onClick={current.reload}>Check Workspace access</Button>
      </div>
    );
  const workspace = current.state.value.detail;
  const agents = (
    <>
      <ul className="sandbox-page__agents">
        {workspace.agents.map((agent) => (
          <li key={agent.id}>
            <div className="sandbox-page__header">
              <h3>
                <ResourceLink route={{ kind: 'agent', agentId: agent.id }} onNavigate={onNavigate}>
                  {agent.title?.trim() || 'Untitled agent'}
                </ResourceLink>
              </h3>
              <span>{statusLabel(agent.status)}</span>
            </div>
            <ResourceAccessSummary value={accessSummaries.get(agent.id)} />
            <p>{channelLabel(channelFacts.get(agent.id))}</p>
          </li>
        ))}
      </ul>
      {workspace.agents.length === 0 ? <p>No authorized Agents in this Workspace.</p> : null}
    </>
  );
  const worktrees = (
    <ul className="shared-hierarchy__worktrees" aria-label="Workspace Worktrees">
      {workspace.worktrees.map((worktree) => (
        <li key={worktree.id}>
          <span>{worktree.name}</span>
          <span>{statusLabel(worktree.status)}</span>
        </li>
      ))}
    </ul>
  );
  if (sandboxId !== undefined)
    return (
      <div>
        <ResourceAccessDetails client={client} access={current.state.value.access} />
        <p>
          {statusLabel(workspace.status)} · {resourceCount(workspace.agent_count, 'Agent')}
        </p>
        {agents}
        {worktrees}
      </div>
    );
  return (
    <section className="sandbox-page">
      <PageHeader
        breadcrumb={[
          {
            label: 'Agent Manager',
            href: routePath({ kind: 'agent_manager' }),
            onSelect: () => onNavigate({ kind: 'agent_manager' }),
          },
          { label: workspace.name },
        ]}
        title={workspace.name}
        badge={<StatusBadge status={workspace.status} live />}
        description={<ResourceAccessDetails client={client} access={current.state.value.access} />}
        actions={<Button onClick={current.reload}>Refresh Workspace access</Button>}
      />
      <section className="sandbox-page__band">
        <h2>Agents</h2>
        {agents}
      </section>
      <section className="sandbox-page__band">
        <h2>Worktrees</h2>
        {worktrees}
      </section>
    </section>
  );
}
