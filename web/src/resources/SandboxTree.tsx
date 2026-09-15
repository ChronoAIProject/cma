import { useMemo, useState } from 'react';
import { ApiError, type AccessibleSandboxSummary } from '@chronoai/cma-sdk';
import type { ResourceViewsClient } from './client';
import type { AppRoute } from '../routing/routes';
import { Button } from '../ui/Button';
import { useAuthorizedRead } from './useAuthorizedRead';
import { loadSandbox } from './loadHierarchy';
import { WorkspaceBranch } from './WorkspaceContents';
import { ResourceLink } from './ResourceLink';
import { statusLabel } from '../vocabulary/status';
import { resourceCount } from '../vocabulary/count';
import { IconChevronDownOutline14, IconChevronRightOutline14 } from '../ui/icons';

export function SandboxTree({
  client,
  organization,
  shared,
  onNavigate,
}: {
  readonly client: ResourceViewsClient;
  readonly organization?: string;
  readonly shared?: boolean;
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const [cursors, setCursors] = useState<readonly (string | undefined)[]>([undefined]);
  const cursor = cursors.at(-1);
  const read = useMemo(
    () => async (signal: AbortSignal) => {
      const page = await client.sandboxes.listAccessible({
        organization,
        shared,
        cursor,
        limit: 50,
        signal,
      });
      if (
        page.next !== undefined &&
        (page.next === '' || (cursor !== undefined && page.next <= cursor))
      )
        throw new Error('Sandbox pagination did not advance. Refresh this scope.');
      let previous = cursor;
      for (const row of page.items) {
        if (previous !== undefined && row.id <= previous)
          throw new Error('Sandbox inventory order changed. Refresh this scope.');
        previous = row.id;
      }
      return { value: page };
    },
    [client, organization, shared, cursor],
  );
  const current = useAuthorizedRead(read);
  return (
    <div className="scoped-navigation__tree">
      <Button size="sm" onClick={current.reload}>
        {shared === true ? 'Refresh shared Sandboxes' : 'Refresh organization'}
      </Button>
      {current.state.status === 'loading' ? (
        <p role="status">
          {shared === true ? 'Loading shared Sandboxes' : 'Loading organization Sandboxes'}
        </p>
      ) : current.state.status === 'error' ? (
        <p role="alert">{current.state.message}</p>
      ) : (
        <>
          {current.state.value.items.length === 0 ? (
            <p>
              {shared === true
                ? 'No shared Sandboxes on this page. Individually shared Agents appear below.'
                : 'No authorized Sandboxes on this page. Agent-only grants appear in the Agent inventory.'}
            </p>
          ) : (
            <ul aria-label={shared === true ? 'Shared Sandboxes' : 'Organization Sandboxes'}>
              {current.state.value.items.map((sandbox) => (
                <SandboxBranch
                  key={sandbox.id}
                  client={client}
                  sandbox={sandbox}
                  onNavigate={onNavigate}
                  onAccessChanged={current.reload}
                />
              ))}
            </ul>
          )}
          <div className="scoped-navigation__pages">
            <Button
              size="sm"
              disabled={cursors.length === 1}
              onClick={() => setCursors((values) => values.slice(0, -1))}
            >
              Previous Sandboxes
            </Button>
            <Button
              size="sm"
              disabled={current.state.value.next === undefined}
              onClick={() => {
                if (current.state.status === 'ready') {
                  const next = current.state.value.next;
                  if (next !== undefined) setCursors((values) => [...values, next]);
                }
              }}
            >
              Next Sandboxes
            </Button>
          </div>
        </>
      )}
    </div>
  );
}

function SandboxBranch({
  client,
  sandbox,
  onNavigate,
  onAccessChanged,
}: {
  readonly client: ResourceViewsClient;
  readonly sandbox: AccessibleSandboxSummary;
  readonly onNavigate: (route: AppRoute) => void;
  readonly onAccessChanged: () => void;
}) {
  const [open, setOpen] = useState(false);
  return (
    <li>
      <Button
        size="sm"
        icon={open ? <IconChevronDownOutline14 /> : <IconChevronRightOutline14 />}
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
      >
        {sandbox.name}
      </Button>
      <p>
        {statusLabel(sandbox.status)} · {resourceCount(sandbox.agent_count, 'Agent')}
      </p>
      <ResourceLink route={{ kind: 'sandbox', sandboxId: sandbox.id }} onNavigate={onNavigate}>
        Open Sandbox details
      </ResourceLink>
      {open ? (
        <SandboxContents
          client={client}
          id={sandbox.id}
          onNavigate={onNavigate}
          onAccessChanged={onAccessChanged}
        />
      ) : null}
    </li>
  );
}

function SandboxContents({
  client,
  id,
  onNavigate,
  onAccessChanged,
}: {
  readonly client: ResourceViewsClient;
  readonly id: string;
  readonly onNavigate: (route: AppRoute) => void;
  readonly onAccessChanged: () => void;
}) {
  const read = useMemo(
    () => async (signal: AbortSignal) => {
      try {
        return await loadSandbox(client, id, signal);
      } catch (error) {
        if (
          !signal.aborted &&
          error instanceof ApiError &&
          [401, 403, 404, 409].includes(error.status)
        )
          onAccessChanged();
        throw error;
      }
    },
    [client, id, onAccessChanged],
  );
  const current = useAuthorizedRead(read);
  return current.state.status === 'loading' ? (
    <p role="status">Loading authorized Workspaces</p>
  ) : current.state.status === 'error' ? (
    <p role="alert">{current.state.message}</p>
  ) : (
    <ul>
      {current.state.value.detail.workspaces.map((workspace) => (
        <WorkspaceBranch
          key={workspace.id}
          client={client}
          id={workspace.id}
          sandboxId={id}
          name={workspace.name}
          onNavigate={onNavigate}
          onAccessChanged={current.reload}
        />
      ))}
      {current.state.value.detail.workspaces.length === 0 ? (
        <li>No authorized Workspaces.</li>
      ) : null}
    </ul>
  );
}
