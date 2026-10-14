import type { EffectiveAccess, ResourceShare } from '@chronoai/cma-sdk';
import type { ResourceViewsClient } from './client';

export interface ShareTarget {
  readonly kind: 'sandbox';
  readonly id: string;
  readonly name: string;
}

export function sharingDeadline(access: EffectiveAccess, started: number): number {
  const deadline = Math.min(
    started + 55_000,
    access.revalidate_at == null ? Infinity : Date.parse(access.revalidate_at),
  );
  if (!access.capabilities.includes('manage_sharing'))
    throw new Error('Only the resource owner can manage these shares. Recipients cannot reshare.');
  if (!Number.isFinite(deadline) || deadline <= Date.now())
    throw new Error('Sharing access has expired. Check current access again.');
  return deadline;
}

export function matchesShare(share: ResourceShare, target: ShareTarget): boolean {
  return share.selector.kind === 'sandbox' && share.selector.sandbox_id === target.id;
}

export async function loadSharing(
  client: ResourceViewsClient,
  target: ShareTarget,
  signal: AbortSignal,
) {
  const started = Date.now();
  const access = await client.resourceAccess.effective(target.kind, target.id, { signal });
  const deadline = sharingDeadline(access, started);
  signal.throwIfAborted();
  const shares = await client.resourceAccess.listShares(target.kind, target.id, { signal });
  if (
    shares.items.some((share) => !matchesShare(share, target) || share.revoked_at !== null) ||
    new Set(shares.items.map((share) => share.id)).size !== shares.items.length
  )
    throw new Error('Sharing information changed. Check current access again.');
  const current = await client.resourceAccess.effective(target.kind, target.id, { signal });
  signal.throwIfAborted();
  const currentDeadline = sharingDeadline(current, started);
  if (current.policy_revision !== access.policy_revision || deadline <= Date.now())
    throw new Error('Sharing access changed. Check current access again.');
  return {
    value: shares.items,
    revalidateAt: new Date(Math.min(deadline, currentDeadline)).toISOString(),
  };
}

export const shareModes = [
  { value: 'view', label: 'View only' },
  { value: 'org_roles', label: 'Use organization roles' },
] as const;

export function shareExposure(mode: ResourceShare['mode']): string {
  const scope =
    'Members can read this Sandbox, every Workspace and Agent within it, and allowed scoped files. This includes future resources in the Sandbox.';
  if (mode === 'view') return `${scope} All recipients have view access only.`;
  return `${scope} Organization admins can also interact, answer approvals and manage the Sandbox and its Agents under current server permissions. Members and viewers cannot interact.`;
}
