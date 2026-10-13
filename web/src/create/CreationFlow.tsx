import type { CmaWebClient } from '../api/client';
import { fromProfileUse } from '../api/creation';
import type { CreateAction } from '../ResourceActions';
import type { ProfileUse } from '../profiles/marketplace/client';
import type { AppRoute } from '../routing/routes';
import type { CreationClient } from './client';
import { NewAgentCard } from './NewAgentCard';
import { NewSandboxCard } from './NewSandboxCard';
import { NewWorkspaceCard } from './NewWorkspaceCard';
import { AgentLifecycleCard } from './AgentLifecycleCard';
import { InlineWorkspaceCard } from './InlineWorkspaceCard';

export type CreationRequest =
  CreateAction | { readonly kind: 'profile_use'; readonly selection: ProfileUse };

export function CreationFlow({
  request,
  client,
  catalog,
  onClose,
  onCreated,
  onChanged,
  canCreateSandbox,
  viewerId,
}: {
  readonly viewerId?: string;
  readonly request: CreationRequest;
  readonly client: CmaWebClient;
  readonly catalog: CreationClient;
  readonly onClose: () => void;
  readonly onCreated: (route: AppRoute) => void;
  readonly onChanged: () => void;
  readonly canCreateSandbox: boolean;
}) {
  const initial = request.kind === 'profile_use' ? fromProfileUse(request.selection) : undefined;
  if (request.kind === 'new_agent' && client.createAgentLifecycle !== undefined) {
    return (
      <AgentLifecycleCard
        key={'workspaceId' in request ? request.workspaceId : 'new-sandbox'}
        client={client}
        viewerId={viewerId}
        workspaceId={'workspaceId' in request ? request.workspaceId : undefined}
        onClose={onClose}
        onCreated={(receipt) => onCreated({ kind: 'agent', agentId: receipt.id })}
      />
    );
  }
  const props = { client, catalog, onClose, initial };
  if (request.kind === 'new_sandbox' || initial?.kind === 'sandbox')
    return (
      <NewSandboxCard
        {...props}
        onCreated={(sandbox) => onCreated({ kind: 'sandbox', sandboxId: sandbox.id })}
      />
    );
  if (request.kind === 'new_workspace')
    return (
      <InlineWorkspaceCard
        client={client}
        sandboxId={request.sandboxId}
        onClose={onClose}
        onCreated={(workspace) => onCreated({ kind: 'sandbox', sandboxId: workspace.sandboxId })}
      />
    );
  if (initial?.kind === 'workspace')
    return (
      <NewWorkspaceCard
        {...props}
        canCreateSandbox={canCreateSandbox}
        onChanged={onChanged}
        sandboxId={undefined}
        onCreated={(workspace) => onCreated({ kind: 'sandbox', sandboxId: workspace.sandboxId })}
      />
    );
  return (
    <NewAgentCard
      {...props}
      canCreateSandbox={canCreateSandbox}
      workspaceId={'workspaceId' in request ? request.workspaceId : undefined}
      onChanged={onChanged}
      onCreated={(agent) => onCreated({ kind: 'agent', agentId: agent.id })}
    />
  );
}
