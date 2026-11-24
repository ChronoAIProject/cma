import { useEffect, useMemo, useState } from 'react';
import type { CmaWebClient } from '../api/client';
import { useAgentAccess } from './agentAccess';
import { useOwnerViews } from './retainedOwnerViews';
import { useOpeningPreparation } from '../agent/useOpeningPreparation';

/** Keep personal routing and retained origin data inside the same access boundary. */
export function useConsoleAgentAccess(
  client: CmaWebClient,
  agentId: string | undefined,
  viewerId: string,
  authEpoch = 0,
) {
  const ownerViews = useOwnerViews();
  const authority = useMemo(() => ({ client, authEpoch }), [client, authEpoch]);
  const agentAccess = useAgentAccess(client.resourceViews, agentId, viewerId, authority);
  // A returning personal view already owns a transport. Its reopen path checks
  // identity itself; preparing another selection would add competing reads.
  const [opening, setOpening] = useState({
    client,
    agentId,
    viewerId,
    retained: agentAccess.personalReady,
  });
  if (opening.client !== client || opening.agentId !== agentId || opening.viewerId !== viewerId)
    setOpening({ client, agentId, viewerId, retained: agentAccess.personalReady });
  const accessUncertain = agentAccess.personalReadOnly;
  useEffect(() => {
    if (
      agentId !== undefined &&
      ((agentAccess.state.status === 'error' && !agentAccess.state.transient) ||
        (agentAccess.state.status === 'ready' &&
          agentAccess.state.value.detail.access === 'shared'))
    )
      ownerViews?.origins.delete(agentId);
  }, [agentId, agentAccess.state, ownerViews]);
  const shared =
    agentAccess.state.status === 'ready' && agentAccess.state.value.detail.access === 'shared'
      ? agentAccess.state.value.detail
      : undefined;
  const personalAgentId =
    client.resourceViews === undefined || agentAccess.personalReady ? agentId : undefined;
  const detail = agentAccess.state.status === 'ready' ? agentAccess.state.value.detail : undefined;
  const retired = detail?.access !== 'shared' && detail?.sandbox.status === 'retired';
  useOpeningPreparation(
    client,
    agentId,
    viewerId,
    personalAgentId !== undefined && !accessUncertain,
    opening.retained ||
      shared !== undefined ||
      agentAccess.state.status === 'error' ||
      (client.resourceViews !== undefined && detail === undefined) ||
      retired,
    agentAccess.personalLineage,
  );
  return { agentAccess, accessUncertain, shared, personalAgentId };
}
