import { useEffect, useState } from 'react';
import type { CmaWebClient } from '../api/client';
import { useAgentAccess } from './agentAccess';
import { useOwnerViews } from './retainedOwnerViews';
import { useOpeningPreparation } from '../agent/useOpeningPreparation';
import { prepareAgentOpeningConnections } from '../connections/preparation';

/** Keep personal routing and retained origin data inside the same access boundary. */
export function useConsoleAgentAccess(
  client: CmaWebClient,
  agentId: string | undefined,
  viewerId: string,
) {
  const ownerViews = useOwnerViews();
  const agentAccess = useAgentAccess(client.resourceViews, agentId, viewerId, client);
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
  const connectionsRefused = shared !== undefined || agentAccess.state.status === 'error';
  useEffect(() => {
    if (
      client.resourceViews === undefined ||
      agentId === undefined ||
      opening.retained ||
      connectionsRefused
    )
      return;
    // The connection endpoint authenticates and authorizes the exact target.
    // Its result is consumed only by the subsequently authorized personal view.
    const connections = prepareAgentOpeningConnections(client, agentId, viewerId);
    return () => connections.release();
  }, [client, agentId, viewerId, opening.retained, connectionsRefused]);
  useOpeningPreparation(
    client,
    agentId,
    viewerId,
    personalAgentId !== undefined && !accessUncertain,
    opening.retained || shared !== undefined || agentAccess.state.status === 'error',
    agentAccess.personalLineage,
  );
  return { agentAccess, accessUncertain, shared, personalAgentId };
}
