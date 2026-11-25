import type { AgentDetail, SandboxDetail, WorkspaceDetail } from '../api/types';
import type { AppRoute } from '../routing/routes';
import type { TreeAction, TreePermissions } from '../shell/Tree';
import { agentDisplayTitle } from '../agent/presentation';
import { ResourceLink } from '../resources/ResourceLink';
import type { SandboxContents } from '../resources/retainedOwnerViews';
import { Alert } from '../ui/Alert';
import { SandboxChildBadge } from './SandboxChildBadge';
import { Button } from '../ui/Button';
import { IconPlusOutline16 } from '../ui/icons';
import { SkeletonRows } from '../ui/Skeleton';
import { sandboxChildrenReadable } from './lifecycle';

function permissionLabel(level: string): string {
  return level === 'guarded' ? 'Guarded' : level === 'autonomous' ? 'Autonomous' : 'Not reported';
}

function WorkspaceAgents({
  sandbox,
  workspace,
  details,
  onNavigate,
}: {
  readonly sandbox: SandboxDetail;
  readonly workspace: WorkspaceDetail;
  readonly details: readonly AgentDetail[];
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const agents = workspace.agents.filter((agent) => agent.status !== 'deleted');
  if (agents.length === 0) return <span className="sandbox-table__quiet">No agents</span>;
  return (
    <ul className="sandbox-table__agents">
      {agents.map((agent) => {
        const detail = details.find((item) => item.id === agent.id);
        return (
          <li key={agent.id}>
            <span className="sandbox-table__agent">
              <ResourceLink route={{ kind: 'agent', agentId: agent.id }} onNavigate={onNavigate}>
                {agentDisplayTitle(agent)}
              </ResourceLink>
              <SandboxChildBadge sandbox={sandbox} status={agent.status} agent={agent} />
            </span>
            {detail === undefined ? null : (
              <span className="sandbox-table__quiet">
                <span>{detail.profile.model ?? 'Provider default'}</span> ·{' '}
                <span>{permissionLabel(detail.profile.permissionLevel)}</span>
              </span>
            )}
          </li>
        );
      })}
    </ul>
  );
}

/**
 * The Workspaces tab: each Workspace with its path and Agents, and where a
 * new Agent can start — the shared style contracts
 *
 * New agent here names its Workspace, so the Agent starts beside the code the
 * owner chose rather than in a new Workspace of its own.
 */
export function SandboxWorkspaces({
  sandbox,
  contents,
  permissions,
  createReason,
  onRetry,
  onAction,
  onNavigate,
}: {
  readonly sandbox: SandboxDetail;
  readonly contents?: SandboxContents;
  readonly permissions: TreePermissions;
  /** Why no Workspace or Agent can start in this Sandbox now, if one cannot. */
  readonly createReason?: string;
  readonly onRetry: () => void;
  readonly onAction: (action: TreeAction) => void;
  readonly onNavigate: (route: AppRoute) => void;
}) {
  return (
    <section className="sandbox-page__band" aria-label="Workspaces">
      <div className="sandbox-page__toolbar">
        <h2>Workspaces</h2>
        {permissions.createWorkspace ? (
          <Button
            size="sm"
            icon={<IconPlusOutline16 />}
            disabled={createReason !== undefined}
            title={createReason}
            onClick={() => onAction({ kind: 'new_workspace', sandboxId: sandbox.id })}
          >
            New workspace
          </Button>
        ) : null}
      </div>
      {!sandboxChildrenReadable(sandbox) ? (
        <p>
          {['retiring', 'retired'].includes(sandbox.status)
            ? 'Workspaces are no longer available in this Sandbox.'
            : 'Workspaces appear here when the Sandbox is ready.'}
        </p>
      ) : contents === undefined ? (
        <SkeletonRows rows={2} label="Loading workspaces" />
      ) : (
        <>
          {contents.failed ? (
            <Alert
              tone="warning"
              actions={
                <Button size="sm" onClick={onRetry}>
                  Try again
                </Button>
              }
            >
              Some workspace or agent details are unavailable.
            </Alert>
          ) : null}
          {contents.workspaces.length === 0 ? (
            contents.failed ? null : (
              <p>No workspaces</p>
            )
          ) : (
            <div className="sandbox-table" role="region" aria-label="Workspace list" tabIndex={0}>
              <table>
                <thead>
                  <tr>
                    <th scope="col">Workspace</th>
                    <th scope="col">Path</th>
                    <th scope="col">Agents</th>
                    <th scope="col" className="sandbox-table__actions">
                      Actions
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {contents.workspaces.map((workspace) => {
                    const reason =
                      createReason ??
                      (workspace.status === 'ready' ? undefined : 'The Workspace is not ready.');
                    return (
                      <tr key={workspace.id}>
                        <th scope="row">
                          <span className="sandbox-table__agent">
                            <ResourceLink
                              route={{ kind: 'workspace', workspaceId: workspace.id }}
                              onNavigate={onNavigate}
                            >
                              {workspace.name}
                            </ResourceLink>
                            <SandboxChildBadge sandbox={sandbox} status={workspace.status} />
                          </span>
                        </th>
                        <td>
                          <code>{workspace.path}</code>
                        </td>
                        <td>
                          {workspace.status === 'removing' ? null : (
                            <WorkspaceAgents
                              sandbox={sandbox}
                              workspace={workspace}
                              details={contents.agents}
                              onNavigate={onNavigate}
                            />
                          )}
                        </td>
                        <td className="sandbox-table__actions">
                          {permissions.createAgent ? (
                            <Button
                              size="sm"
                              icon={<IconPlusOutline16 />}
                              disabled={reason !== undefined}
                              title={reason}
                              aria-label={`New agent in ${workspace.name}`}
                              onClick={() =>
                                onAction({ kind: 'new_agent', workspaceId: workspace.id })
                              }
                            >
                              New agent here
                            </Button>
                          ) : null}
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          )}
        </>
      )}
    </section>
  );
}
