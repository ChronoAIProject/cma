import { useEffect, useRef } from 'react';
import type { CmaWebClient } from '../api/client';
import type { AgentOpeningPreparation } from './transport/preparation';
import type { AgentLoadLineage } from './loadAgent';
import { prepareOpeningReads } from './transport/openingReads';

/** Only classified personal routes admit protected reads while selection finishes. */
export function useOpeningPreparation(
  client: CmaWebClient,
  agentId: string | undefined,
  owner: string,
  authorized: boolean,
  refused: boolean,
  lineage?: AgentLoadLineage,
) {
  const transport = client.agentTransport;
  const active = useRef<AgentOpeningPreparation | undefined>(undefined);
  useEffect(() => {
    if (!transport || !agentId || refused) return;
    const preparation = transport.prepareOpening(agentId, owner);
    active.current = preparation;
    return () => {
      preparation.release();
      if (active.current === preparation) active.current = undefined;
    };
  }, [client, transport, agentId, owner, refused]);
  // Register before the controller's load effect so only a classified personal
  // view consumes this selection. authorizeLoad still makes a fresh request.
  useEffect(() => {
    if (!authorized || refused || !agentId || !active.current) return;
    active.current.handoff();
    const reads = prepareOpeningReads(client, agentId, active.current, lineage);
    return () => reads.release();
  }, [client, transport, agentId, owner, authorized, refused, lineage]);
}
