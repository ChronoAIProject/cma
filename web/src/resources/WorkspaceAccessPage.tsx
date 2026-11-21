import type { CmaWebClient } from '../api/client';
import type { AppRoute } from '../routing/routes';
import type { TreeAction, TreePermissions } from '../shell/Tree';
import { OwnerWorkspacePage } from './OwnerWorkspacePage';
import { WorkspaceContents } from './WorkspaceContents';

/**
 * `/workspaces/:id`: the owner's Workspace page, or the shared projection.
 *
 * The same classification the Sandbox route uses (`SandboxAccessPage`): the
 * accessible read decides whether the viewer owns the Workspace, and only an
 * owner gets the page that offers owner actions. Without the accessible-read
 * client there is nothing shared to project, so the owner reads decide alone
 * and refuse anybody else.
 */
export function WorkspaceAccessPage({
  client,
  id,
  revision,
  permissions,
  busy,
  onAction,
  onNavigate,
}: {
  readonly client: CmaWebClient;
  readonly id: string;
  readonly revision: number;
  readonly permissions: TreePermissions;
  readonly busy: boolean;
  readonly onAction: (action: TreeAction) => void;
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const owner = (
    <OwnerWorkspacePage
      client={client}
      workspaceId={id}
      revision={revision}
      permissions={permissions}
      busy={busy}
      onAction={onAction}
      onNavigate={onNavigate}
    />
  );
  if (client.resourceViews === undefined) return owner;
  return (
    <WorkspaceContents
      client={client.resourceViews}
      id={id}
      personal={owner}
      onNavigate={onNavigate}
    />
  );
}
