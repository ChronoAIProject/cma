import { useMemo, useState, type ReactNode } from 'react';
import { ApiError, type AccessibleSandboxSummary } from '@chronoai/cma-sdk';
import type { ResourceViewsClient } from './client';
import type { AppRoute } from '../routing/routes';
import { Picker } from '../ui/Picker';
import { Button } from '../ui/Button';
import { useAuthorizedRead } from './useAuthorizedRead';
import { loadSandbox } from './loadHierarchy';
import { WorkspaceBranch } from './WorkspaceContents';
import { ResourceLink } from './ResourceLink';
import { statusLabel } from '../vocabulary/status';
import { resourceCount } from '../vocabulary/count';
import { IconChevronDownOutline14, IconChevronRightOutline14 } from '../ui/icons';
import './ScopedNavigation.css';
import { scopeRoute, scopeValue } from './scope';
import { SharedAgentNavigation } from './SharedAgentNavigation';

export function ScopedNavigation({
  client,
  personal,
  scope,
  onScopeChange,
  onNavigate,
}: {
  readonly client?: ResourceViewsClient;
  readonly personal: ReactNode;
  readonly scope: string;
  readonly onScopeChange: (scope: string) => void;
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const read = useMemo(
    () =>
      client === undefined
        ? undefined
        : async (signal: AbortSignal) => ({
            value: (await client.resourceAccess.organizations({ signal })).items,
          }),
    [client],
  );
  const current = useAuthorizedRead(read);
  if (client === undefined) return personal;
  const { organization, shared } = scopeRoute(scope);
  const organizations = current.state.status === 'ready' ? current.state.value : [];
  const selected = organizations.find((choice) => choice.id === organization);
  const options = [
    { value: '', label: 'Personal' },
    { value: 'shared', label: 'Shared with me' },
    ...organizations.map((choice, index) => ({
      value: scopeValue(choice.id),
      label: choice.display_name?.trim() || `Organization ${index + 1}`,
    })),
  ];
  if (organization !== undefined && selected === undefined)
    options.push({ value: scopeValue(organization), label: 'Selected organization' });
  return (
    <div className="scoped-navigation">
      <Picker label="Navigation scope" value={scope} options={options} onChange={onScopeChange} />
      {current.state.status === 'error' ? (
        <p>Organization choices are unavailable. Personal navigation remains available.</p>
      ) : null}
      {shared === true ? (
        <SharedAgentNavigation client={client} onNavigate={onNavigate} />
      ) : organization === undefined ? (
        personal
      ) : (
        <>
          <p>
            {selected?.display_name?.trim() || 'Organization'}
            {selected === undefined ? '' : ` · Organization ${selected.role}`}
          </p>
          <ResourceLink route={{ kind: 'inventory', organization }} onNavigate={onNavigate}>
            All authorized Agents
          </ResourceLink>
          <OrganizationTree
            key={organization}
            client={client}
            organization={organization}
            onNavigate={onNavigate}
          />
        </>
      )}
    </div>
  );
}

function OrganizationTree({
  client,
  organization,
  onNavigate,
}: {
  readonly client: ResourceViewsClient;
  readonly organization: string;
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const [cursors, setCursors] = useState<readonly (string | undefined)[]>([undefined]);
  const cursor = cursors.at(-1);
  const read = useMemo(
    () => async (signal: AbortSignal) => {
      const page = await client.sandboxes.listAccessible({
        organization,
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
    [client, organization, cursor],
  );
  const current = useAuthorizedRead(read);
  return (
    <div className="scoped-navigation__tree">
      <Button size="sm" onClick={current.reload}>
        Refresh organization
      </Button>
      {current.state.status === 'loading' ? (
        <p role="status">Loading organization Sandboxes</p>
      ) : current.state.status === 'error' ? (
        <p role="alert">{current.state.message}</p>
      ) : (
        <>
          {current.state.value.items.length === 0 ? (
            <p>
              No authorized Sandboxes on this page. Agent-only grants appear in the Agent inventory.
            </p>
          ) : (
            <ul aria-label="Organization Sandboxes">
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
