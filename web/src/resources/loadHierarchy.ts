import type {
  AccessibleSandboxDetail,
  AccessibleWorkspaceDetail,
  EffectiveAccess,
} from '@cma/client';
import type { ResourceViewsClient } from './client';

function deadline(access: EffectiveAccess, started: number) {
  const expires = Math.min(
    started + 55_000,
    access.revalidate_at == null ? Infinity : Date.parse(access.revalidate_at),
  );
  if (
    !access.capabilities.includes('view_details') ||
    !Number.isFinite(expires) ||
    expires <= Date.now()
  )
    throw new Error('Current resource access is unavailable.');
  return expires;
}

async function authorized<Value>(
  client: ResourceViewsClient,
  kind: 'sandbox' | 'workspace',
  id: string,
  signal: AbortSignal,
  read: () => Promise<Value>,
) {
  const started = Date.now();
  const initial = await client.resourceAccess.effective(kind, id, { signal });
  const originalDeadline = deadline(initial, started);
  signal.throwIfAborted();
  const detail = await read();
  signal.throwIfAborted();
  const current = await client.resourceAccess.effective(kind, id, { signal });
  const expires = Math.min(originalDeadline, deadline(current, started));
  signal.throwIfAborted();
  if (initial.policy_revision !== current.policy_revision || expires <= Date.now())
    throw new Error('Resource access changed. Refresh to check current access.');
  return { value: { detail, access: current }, revalidateAt: new Date(expires).toISOString() };
}

function unique(ids: readonly string[]) {
  if (new Set(ids).size !== ids.length)
    throw new Error('Resource hierarchy contains repeated identities.');
}

export function loadSandbox(client: ResourceViewsClient, id: string, signal: AbortSignal) {
  return authorized(client, 'sandbox', id, signal, async (): Promise<AccessibleSandboxDetail> => {
    const detail = await client.sandboxes.readAccessible(id, { signal });
    if (detail.id !== id || detail.workspaces.some((workspace) => workspace.sandbox_id !== id))
      throw new Error('Sandbox hierarchy identity changed.');
    unique(detail.workspaces.map((workspace) => workspace.id));
    return detail;
  });
}

export function loadWorkspace(
  client: ResourceViewsClient,
  id: string,
  signal: AbortSignal,
  sandboxId?: string,
) {
  return authorized(
    client,
    'workspace',
    id,
    signal,
    async (): Promise<AccessibleWorkspaceDetail> => {
      const detail = await client.workspaces.readAccessible(id, { signal });
      if (
        detail.id !== id ||
        (sandboxId !== undefined && detail.sandbox_id !== sandboxId) ||
        [...detail.worktrees, ...detail.agents].some(
          (child) => child.workspace_id !== id || child.sandbox_id !== detail.sandbox_id,
        )
      )
        throw new Error('Workspace hierarchy identity changed.');
      unique(detail.worktrees.map((worktree) => worktree.id));
      unique(detail.agents.map((agent) => agent.id));
      unique(detail.agents.map((agent) => agent.worktree_id));
      if (
        detail.agents.some(
          (agent) =>
            !detail.worktrees.some(
              (worktree) => worktree.id === agent.worktree_id && worktree.agent_id === agent.id,
            ),
        )
      )
        throw new Error('Workspace Agent assignment changed.');
      return detail;
    },
  );
}
