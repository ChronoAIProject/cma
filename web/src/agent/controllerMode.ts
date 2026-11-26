import { useMemo, useState } from 'react';
import type { CmaWebClient } from '../api/client';
import type { AgentControllerOptions } from './controllerTypes';
import { deriveRetiredPresentation } from './retiredPresentation';
import { useRetainedHistory } from './retainedHistory';

export function useControllerMode(
  client: CmaWebClient,
  agentId: string | undefined,
  options: AgentControllerOptions,
) {
  const { viewerId, authEpoch } = options;
  const scope = useMemo(
    () => ({ client, viewerId, authEpoch, agentId }),
    [client, viewerId, authEpoch, agentId],
  );
  const observed = deriveRetiredPresentation(options.selectedRetired, {
    viewerId,
    authEpoch,
    agentId,
    authorized: options.suspended !== true,
  });
  const [previous, setPrevious] = useState({ scope, retired: false });
  // Losing fresh authority hides history; it cannot reopen this visit's retired runtime.
  const retired = observed.kind === 'retired' || (previous.scope === scope && previous.retired);
  if (previous.scope !== scope || previous.retired !== retired) setPrevious({ scope, retired });
  const suspended = options.suspended === true || retired;
  const {
    agentId: lineageAgentId,
    workspaceId: lineageWorkspaceId,
    sandboxId: lineageSandboxId,
  } = options.authorizedLineage ?? {};
  const lineage = useMemo(
    () =>
      !suspended &&
      viewerId !== undefined &&
      lineageAgentId === agentId &&
      lineageAgentId !== undefined &&
      lineageWorkspaceId !== undefined &&
      lineageSandboxId !== undefined
        ? { agentId: lineageAgentId, workspaceId: lineageWorkspaceId, sandboxId: lineageSandboxId }
        : undefined,
    [agentId, suspended, viewerId, lineageAgentId, lineageWorkspaceId, lineageSandboxId],
  );
  const history = useRetainedHistory(client, observed.kind === 'retired' ? observed : undefined);
  return { suspended, lineage, retained: retired ? history : undefined };
}
