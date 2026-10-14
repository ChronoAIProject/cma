import { useCallback, useEffect, useRef } from 'react';
import type { AgentLifecycleOperationView, CmaWebClient } from '../api/client';
import type { AgentTransportPreparation } from '../agent/transport/preparation';

/** Start selection only after this retained operation names its actual Agent. */
export function useLifecycleTransport(
  client: CmaWebClient,
  operation: AgentLifecycleOperationView | undefined,
  receiptId: string | undefined,
  viewerId: string | undefined,
) {
  const operationId = operation?.id;
  const agentId = operation?.agent_id;
  const eligible =
    operationId === receiptId &&
    (operation?.status === 'running' || operation?.status === 'succeeded');
  const active = useRef<AgentTransportPreparation | undefined>(undefined);
  useEffect(() => {
    if (!eligible || !operationId || !agentId || !viewerId) return;
    const preparation = client.agentTransport?.prepare(agentId, viewerId);
    active.current = preparation;
    return () => {
      preparation?.release();
      if (active.current === preparation) active.current = undefined;
    };
  }, [agentId, client, eligible, operationId, viewerId]);
  return useCallback(() => active.current?.handoff(), []);
}
