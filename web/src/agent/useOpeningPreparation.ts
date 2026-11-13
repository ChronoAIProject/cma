import { useEffect, useRef } from 'react';
import type { CmaWebClient } from '../api/client';
import type { AgentTransportPreparation } from './transport/preparation';

/** Selection may overlap access classification; protected reads still wait for both. */
export function useOpeningPreparation(
  client: CmaWebClient,
  agentId: string | undefined,
  owner: string,
  authorized: boolean,
  refused: boolean,
) {
  const transport = client.agentTransport;
  const active = useRef<AgentTransportPreparation | undefined>(undefined);
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
    if (authorized && !refused) active.current?.handoff();
  }, [client, transport, agentId, owner, authorized, refused]);
}
