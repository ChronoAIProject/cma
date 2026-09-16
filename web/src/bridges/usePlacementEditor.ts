import { useMemo, useState } from 'react';
import type {
  BridgeCapabilityCatalog,
  BridgeConfiguration,
  BridgePlacement,
  BridgePlatform,
} from '@chronoai/cma-sdk';
import type { BridgeViewsClient } from './client';
import { useAuthorizedRead } from '../resources/useAuthorizedRead';
import { isShared, loadPlacementAccess, type SharedPlacement } from './placementAccess';
import { loadRetainedTarget, retainedTargetComplete } from './retainedTargetAccess';

export function placementUnavailable(
  catalog: BridgeCapabilityCatalog | undefined,
  platform: BridgePlatform,
  kind: SharedPlacement['kind'],
): string | undefined {
  const mode = catalog?.placements?.find((item) => item.kind === kind);
  if (mode === undefined) return 'Shared placement availability could not be verified.';
  if (mode.availability === 'runtime_unavailable')
    return 'Shared placement requires runtime support that is unavailable.';
  if (mode.availability === 'continuity_unverified')
    return 'Shared placement is waiting for continuity verification.';
  if (mode.availability !== 'available')
    return 'Shared placement availability could not be verified.';
  const connector = catalog?.connectors.find((item) => item.platform === platform);
  if (
    catalog?.connection_validation_available !== true ||
    connector?.evidence !== 'contract_tested' ||
    !connector.stages.management ||
    !connector.stages.allocation ||
    !connector.stages.execution
  )
    return 'Shared placement is disabled for this connection.';
  return undefined;
}

export function usePlacementEditor(
  client: BridgeViewsClient,
  config: BridgeConfiguration | undefined,
  organization?: string,
  original?: BridgePlacement,
) {
  const enabled = config !== undefined;
  const read = useMemo(
    () =>
      !enabled
        ? undefined
        : async (signal: AbortSignal) => ({
            value: await client.bridges.capabilities({ organization, signal }),
          }),
    [client, organization, enabled],
  );
  const capabilities = useAuthorizedRead(read);
  const catalog = capabilities.state.status === 'ready' ? capabilities.state.value : undefined;
  const placement = config?.placement;
  const shared = placement !== undefined && isShared(placement) ? placement : undefined;
  const reason =
    shared === undefined
      ? undefined
      : placementUnavailable(catalog, config!.binding.platform, shared.kind);
  const sandboxId = shared?.sandbox_id ?? '';
  const workspaceId =
    shared?.kind === 'worktree_agent_per_partition' ? shared.workspace_id : undefined;
  const readable =
    shared !== undefined && reason === undefined && sandboxId !== '' && workspaceId !== '';
  const accessRead = useMemo(
    () =>
      !readable
        ? undefined
        : (signal: AbortSignal) =>
            loadPlacementAccess(client, sandboxId, workspaceId, organization, signal),
    [client, sandboxId, workspaceId, organization, readable],
  );
  const access = useAuthorizedRead(accessRead);
  const key = JSON.stringify(placement ?? null);
  const retained = original !== undefined && key === JSON.stringify(original);
  const retainedTarget = placement?.kind === 'existing_agent' ? placement.target : undefined;
  const targetRead = useMemo(
    () =>
      organization === undefined ||
      retainedTarget === undefined ||
      !retainedTargetComplete(retainedTarget)
        ? undefined
        : (signal: AbortSignal) => loadRetainedTarget(client, retainedTarget, organization, signal),
    [client, organization, retainedTarget],
  );
  const retainedAccess = useAuthorizedRead(targetRead);
  const [trust, setTrust] = useState<{ key: string; checked: boolean }>();
  const trustChecked = trust !== undefined && trust.key === key ? trust.checked : retained;
  return {
    capabilities,
    catalog,
    access,
    reason,
    trustChecked,
    retainedAccess,
    setTrust: (checked: boolean) => setTrust({ key, checked }),
    // An unchanged retained placement can accompany unrelated settings edits.
    // New placement intent requires fresh evidence, resource access and consent.
    canSave:
      (shared === undefined &&
        (organization === undefined ||
          retainedTarget === undefined ||
          retained ||
          retainedAccess.state.status === 'ready')) ||
      (retained && trustChecked) ||
      (reason === undefined && access.state.status === 'ready' && trustChecked),
  };
}
export type PlacementEditor = ReturnType<typeof usePlacementEditor>;
