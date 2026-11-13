import { useEffect } from 'react';
import type { CmaWebClient } from '../api/client';
import { useAgentAccess } from './agentAccess';
import { useOwnerViews } from './retainedOwnerViews';

/** Keep personal routing and retained origin data inside the same access boundary. */
export function useConsoleAgentAccess(
  client: CmaWebClient,
  agentId: string | undefined,
  viewerId: string,
) {
  const ownerViews = useOwnerViews();
  const agentAccess = useAgentAccess(client.resourceViews, agentId, viewerId, client);
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
  return { agentAccess, accessUncertain, shared, personalAgentId };
}
