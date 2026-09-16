import { ValidationError, type BridgeConfiguration, type BridgePlacement } from '@chronoai/cma-sdk';
import type { BridgeViewsClient } from './client';
import { isShared, loadPlacementAccess } from './placementAccess';
import { placementUnavailable } from './usePlacementEditor';
import { loadRetainedTarget } from './retainedTargetAccess';

/** Recheck new placement intent at submission. Retained receipt recovery is separate. */
export async function verifyPlacementForSave(
  client: BridgeViewsClient,
  config: BridgeConfiguration,
  organization: string | undefined,
  original: BridgePlacement | undefined,
  signal: AbortSignal | undefined,
) {
  const placement = config.placement;
  if (JSON.stringify(placement) === JSON.stringify(original)) return;
  if (!isShared(placement)) {
    if (placement.kind === 'existing_agent' && organization !== undefined) {
      try {
        await loadRetainedTarget(
          client,
          placement.target,
          organization,
          signal ?? new AbortController().signal,
        );
      } catch {
        throw new ValidationError('Current retained Agent access or ancestry changed.');
      }
    }
    return;
  }
  try {
    const check = async () => {
      const catalog = await client.bridges.capabilities({ organization, signal });
      if (placementUnavailable(catalog, config.binding.platform, placement.kind) !== undefined)
        throw new Error('Placement is unavailable');
    };
    await check();
    const controller = signal ?? new AbortController().signal;
    const access = await loadPlacementAccess(
      client,
      placement.sandbox_id,
      placement.kind === 'worktree_agent_per_partition' ? placement.workspace_id : undefined,
      organization,
      controller,
    );
    await check();
    if (Date.parse(access.revalidateAt) <= Date.now()) throw new Error('Resource access expired');
    signal?.throwIfAborted();
  } catch {
    // No mutation was sent. Let the editor refresh instead of retaining an
    // ambiguous mutation key for a failed read-only eligibility check.
    throw new ValidationError('Shared placement availability or resource access changed.');
  }
}
