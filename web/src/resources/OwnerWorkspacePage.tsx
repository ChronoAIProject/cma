import { useEffect, useId, useRef } from 'react';
import type { CmaWebClient } from '../api/client';
import type { SandboxDetail, WorkspaceDetail } from '../api/types';
import { agentDisplayTitle } from '../agent/presentation';
import { routePath, type AppRoute } from '../routing/routes';
import { sandboxChildrenAvailable, sandboxUnavailableNotice } from '../sandbox/lifecycle';
import { sandboxCreateReason } from '../sandbox/sandboxHeaderActions';
import type { TreeAction, TreePermissions } from '../shell/Tree';
import { Alert } from '../ui/Alert';
import { SandboxChildBadge } from '../sandbox/SandboxChildBadge';
import { StatusBadge } from '../ui/Badge';
import { Button } from '../ui/Button';
import { IconPlusOutline16, IconTrashOutline16 } from '../ui/icons';
import type { MenuEntry } from '../ui/Menu';
import { MoreActions } from '../ui/MoreActions';
import { PageHeader } from '../ui/PageHeader';
import { Presence } from '../ui/Presence';
import { SkeletonRows } from '../ui/Skeleton';
import { useDocumentTitle } from '../ui/useDocumentTitle';
import { ResourceLink } from './ResourceLink';
import { useOwnerWorkspace } from './useOwnerWorkspace';
import '../sandbox/SandboxPage.css';

/** Why the Workspace cannot be deleted now, in the order an owner can act on it. */
function deleteReason(
  sandbox: SandboxDetail,
  workspace: WorkspaceDetail,
  busy: boolean,
  stale: boolean,
) {
  if (workspace.status === 'removing') return 'Deletion is already in progress.';
  if (workspace.status === 'removed') return 'This Workspace has been deleted.';
  return sandboxCreateReason(sandbox, { busy, stale, needsConnections: false });
}

function WorkspaceFacts({ workspace }: { readonly workspace: WorkspaceDetail }) {
  const { source } = workspace;
  // Only a web address becomes a link; anything else stays visible as text.
  const linkable = source.type === 'git_repository' && /^https?:\/\//i.test(source.url);
  return (
    <section className="sandbox-page__band">
      <h2>Details</h2>
      <dl>
        <div>
          <dt>Path</dt>
          <dd>
            <code>{workspace.path}</code>
          </dd>
        </div>
        <div>
          <dt>Source</dt>
          <dd>
            {source.type === 'empty' ? (
              'Empty directory'
            ) : linkable ? (
              <a href={source.url} target="_blank" rel="noopener noreferrer">
                {source.url}
              </a>
            ) : (
              <code>{source.url}</code>
            )}
          </dd>
        </div>
        <div>
          <dt>Branch or ref</dt>
          <dd>
            {source.type === 'empty' ? (
              'None'
            ) : source.reference === undefined ? (
              'Default branch'
            ) : (
              <code>{source.reference}</code>
            )}
          </dd>
        </div>
        <div>
          <dt>Resolved commit</dt>
          <dd>
            {workspace.resolvedCommit === undefined ? (
              'Not reported'
            ) : (
              <code>{workspace.resolvedCommit}</code>
            )}
          </dd>
        </div>
      </dl>
    </section>
  );
}

function WorkspaceAgents({
  sandbox,
  workspace,
  onNavigate,
}: {
  readonly sandbox: SandboxDetail;
  readonly workspace: WorkspaceDetail;
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const agents = workspace.agents.filter((agent) => agent.status !== 'deleted');
  return (
    <section className="sandbox-page__band" aria-label="Agents and Worktrees">
      <h2>Agents and Worktrees</h2>
      {agents.length === 0 ? (
        <p>No agents</p>
      ) : (
        <div className="sandbox-table" role="region" aria-label="Agent list" tabIndex={0}>
          <table>
            <thead>
              <tr>
                <th scope="col">Agent</th>
                <th scope="col">Status</th>
                <th scope="col">Worktree</th>
                <th scope="col">Worktree status</th>
              </tr>
            </thead>
            <tbody>
              {agents.map((agent) => {
                // Each Worktree has exactly one Agent, so the pair shares a row.
                const worktree = workspace.worktrees.find((row) => row.id === agent.worktreeId);
                return (
                  <tr key={agent.id}>
                    <th scope="row">
                      <ResourceLink
                        route={{ kind: 'agent', agentId: agent.id }}
                        onNavigate={onNavigate}
                      >
                        {agentDisplayTitle(agent)}
                      </ResourceLink>
                    </th>
                    <td>
                      <SandboxChildBadge sandbox={sandbox} status={agent.status} agent={agent} />
                    </td>
                    <td>
                      {worktree === undefined ? (
                        'Not reported'
                      ) : (
                        <code title={worktree.name}>{worktree.branch}</code>
                      )}
                    </td>
                    <td>
                      {worktree === undefined ? null : <StatusBadge status={worktree.status} />}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

/**
 * The owner's Workspace page — the shared style contracts
 *
 * Owners used to land on the projection made for shared readers, which
 * said "Your access: Owner" and offered nothing an owner does here. This one
 * places the Workspace in its Sandbox, states where its code came from, pairs
 * each Agent with its Worktree, and starts a new Agent in this Workspace.
 */
export function OwnerWorkspacePage({
  client,
  workspaceId,
  revision,
  permissions,
  busy,
  onAction,
  onNavigate,
}: {
  readonly client: CmaWebClient;
  readonly workspaceId: string;
  readonly revision: number;
  readonly permissions: TreePermissions;
  readonly busy: boolean;
  readonly onAction: (action: TreeAction) => void;
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const { state, revalidating, retry } = useOwnerWorkspace(client, workspaceId, revision);
  const heading = useRef<HTMLHeadingElement>(null);
  const reasonId = useId();
  useDocumentTitle(state.status === 'ready' ? state.workspace.name : 'Workspace');
  useEffect(() => {
    heading.current?.focus();
  }, [workspaceId, state.status]);
  if (state.status === 'loading')
    return (
      <section className="sandbox-page" aria-busy="true">
        <h1 ref={heading} tabIndex={-1}>
          Workspace
        </h1>
        <SkeletonRows rows={4} label="Loading Workspace" />
      </section>
    );
  if (state.status === 'error')
    return (
      <section className="sandbox-page" role="alert">
        <h1 ref={heading} tabIndex={-1}>
          Workspace unavailable
        </h1>
        <p>{state.message}</p>
        <Button onClick={retry}>Try again</Button>
      </section>
    );
  const { workspace, sandbox, pollError } = state;
  const stale = pollError || revalidating;
  const sandboxRoute = { kind: 'sandbox', sandboxId: sandbox.id } as const;
  const createReason =
    sandboxCreateReason(sandbox, { busy, stale, needsConnections: false }) ??
    (workspace.status === 'ready' ? undefined : 'The Workspace is not ready.');
  const removal = deleteReason(sandbox, workspace, busy, stale);
  const menu: MenuEntry[] = permissions.removeWorkspace
    ? [
        {
          id: 'remove_workspace',
          label: 'Delete workspace',
          icon: <IconTrashOutline16 />,
          danger: true,
          disabled: removal !== undefined,
          reason: removal,
        },
      ]
    : [];
  const notice = sandboxChildrenAvailable(sandbox) ? undefined : sandboxUnavailableNotice(sandbox);
  return (
    <section className="sandbox-page">
      <PageHeader
        breadcrumb={[
          {
            label: sandbox.name,
            href: routePath(sandboxRoute),
            onSelect: () => onNavigate(sandboxRoute),
          },
          { label: workspace.name },
        ]}
        title={workspace.name}
        headingRef={heading}
        badge={<SandboxChildBadge sandbox={sandbox} status={workspace.status} live />}
        actions={
          <>
            {permissions.createAgent ? (
              <Button
                size="sm"
                variant="primary"
                icon={<IconPlusOutline16 />}
                disabled={createReason !== undefined}
                aria-describedby={createReason === undefined ? undefined : reasonId}
                aria-label="New agent in this Workspace"
                onClick={() => onAction({ kind: 'new_agent', workspaceId })}
              >
                New agent
              </Button>
            ) : null}
            <MoreActions
              items={menu}
              onSelect={(id) => {
                if (id === 'remove_workspace') onAction({ kind: 'remove_workspace', workspaceId });
              }}
            />
          </>
        }
      />
      {createReason === undefined || !permissions.createAgent ? null : (
        <div className="sandbox-page__reasons">
          <p id={reasonId}>{createReason}</p>
        </div>
      )}
      <div className="alert-stack" data-sandbox-alerts="">
        <Presence>
          {pollError ? (
            <Alert tone="warning">Progress could not be refreshed. Reconnecting.</Alert>
          ) : null}
        </Presence>
        {workspace.status === 'failed' ? (
          <Alert tone="error" title="This Workspace needs attention.">
            {workspace.failure?.remediation ??
              "Open the Sandbox's Technical details for the recorded failure."}
          </Alert>
        ) : null}
        {notice === undefined ? null : <Alert tone="info">{notice}</Alert>}
      </div>
      <WorkspaceFacts workspace={workspace} />
      <WorkspaceAgents sandbox={sandbox} workspace={workspace} onNavigate={onNavigate} />
    </section>
  );
}
