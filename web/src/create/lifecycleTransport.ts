import { useCallback, useEffect, useRef } from 'react';
import type { AgentLifecycleOperationView, CmaWebClient } from '../api/client';
import type { AgentTransportPreparation } from '../agent/transport/preparation';
import { prepareAgentConnections } from '../connections/preparation';
import { prepareDesktop } from '../agent/desktopPreparation';

/** Start selection only after this retained operation names its actual Agent. */
export function useLifecycleTransport(
  client: CmaWebClient,
  operation: AgentLifecycleOperationView | undefined,
  receiptId: string | undefined,
  viewerId: string | undefined,
  machineIncluded = false,
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
    const connections = preparation && prepareAgentConnections(client, agentId, viewerId);
    const desktop = machineIncluded ? prepareDesktop(client, agentId, viewerId) : undefined;
    const handoff = {
      handoff() {
        preparation?.handoff();
        connections?.handoff();
        desktop?.handoff();
      },
      release() {
        preparation?.release();
        connections?.release();
        desktop?.release();
      },
    };
    active.current = handoff;
    return () => {
      handoff.release();
      if (active.current === handoff) active.current = undefined;
    };
  }, [agentId, client, eligible, machineIncluded, operationId, viewerId]);
  return useCallback(() => active.current?.handoff(), []);
}
