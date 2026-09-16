import type { BridgePlacement, EffectiveAccess } from '@chronoai/cma-sdk';
import type { BridgeViewsClient } from './client';

export type SharedPlacement = Extract<
  BridgePlacement,
  { kind: 'workspace_per_partition' | 'worktree_agent_per_partition' }
>;

export function isShared(placement: BridgePlacement): placement is SharedPlacement {
  return (
    placement.kind === 'workspace_per_partition' ||
    placement.kind === 'worktree_agent_per_partition'
  );
}

export function accessDeadline(
  access: EffectiveAccess,
  organization: string | undefined,
  started: number,
  capabilities: readonly string[] = ['manage_sandbox'],
) {
  const owner = access.owner;
  const sameOwner =
    organization === undefined
      ? owner?.kind === 'person' && owner.is_current_person
      : owner?.kind === 'organization' && owner.organization_id === organization;
  const until = Math.min(
    started + 55_000,
    access.revalidate_at == null ? Infinity : Date.parse(access.revalidate_at),
  );
  if (
    !sameOwner ||
    capabilities.some((capability) => !access.capabilities.includes(capability)) ||
    !Number.isFinite(until) ||
    until <= Date.now()
  )
    throw new Error(
      'The selected resource needs current management access and the same owner as this Bridge.',
    );
  return until;
}

/** Existing public reads verify exact ancestry and current effective authority. */
export async function loadPlacementHierarchy(
  client: BridgeViewsClient,
  sandboxId: string,
  workspaceId: string | undefined,
  organization: string | undefined,
  signal: AbortSignal,
) {
  const started = Date.now();
  const target = workspaceId === undefined ? ['sandbox', sandboxId] : ['workspace', workspaceId];
  const initial = await client.resourceAccess.effective(target[0]!, target[1]!, { signal });
  let until = accessDeadline(initial, organization, started);
  const sandbox = await client.sandboxes.readAccessible(sandboxId, { signal });
  if (sandbox.id !== sandboxId || !['active', 'paused'].includes(sandbox.status))
    throw new Error('Choose an active or paused Sandbox.');
  const workspace =
    workspaceId === undefined
      ? undefined
      : await client.workspaces.readAccessible(workspaceId, { signal });
  if (workspace !== undefined) {
    if (
      workspace.id !== workspaceId ||
      workspace.sandbox_id !== sandboxId ||
      workspace.status !== 'ready'
    )
      throw new Error('The selected Workspace must be ready and belong to this Sandbox.');
  }
  const current = await client.resourceAccess.effective(target[0]!, target[1]!, { signal });
  until = Math.min(until, accessDeadline(current, organization, started));
  signal.throwIfAborted();
  if (initial.policy_revision !== current.policy_revision || until <= Date.now())
    throw new Error('Resource access changed. Check the selected resource again.');
  return { value: { sandbox, workspace }, revalidateAt: new Date(until).toISOString() };
}

export async function loadPlacementAccess(...args: Parameters<typeof loadPlacementHierarchy>) {
  const hierarchy = await loadPlacementHierarchy(...args);
  return { value: true, revalidateAt: hierarchy.revalidateAt };
}
