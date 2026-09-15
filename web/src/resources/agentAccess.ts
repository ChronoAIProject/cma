import { useMemo, useState } from 'react';
import type { AccessibleAgent, EffectiveAccess } from '@chronoai/cma-sdk';
import type { ResourceViewsClient } from './client';
import { useAuthorizedRead, type AuthorizedValue } from './useAuthorizedRead';

export interface AgentAccessValue {
  readonly detail: AccessibleAgent;
  readonly access?: EffectiveAccess;
}

export function useAgentAccess(
  client: ResourceViewsClient | undefined,
  agentId: string | undefined,
) {
  const [personal, setPersonal] = useState<{
    readonly client: ResourceViewsClient;
    readonly agentId: string;
  }>();
  const read = useMemo(
    () =>
      client === undefined || agentId === undefined
        ? undefined
        : async (signal: AbortSignal): Promise<AuthorizedValue<AgentAccessValue>> => {
            const detail = await client.agents.readAccessible(agentId, { signal });
            signal.throwIfAborted();
            if (detail.access !== 'shared') {
              setPersonal({ client, agentId });
              return { value: { detail } };
            }
            const access = await client.resourceAccess.effective('agent', agentId, { signal });
            if (!access.capabilities.includes('view_details'))
              throw new Error('Agent access is unavailable.');
            return { value: { detail, access }, revalidateAt: access.revalidate_at };
          },
    [client, agentId],
  );
  const result = useAuthorizedRead(read);
  // Keep the personal controller during classification refreshes. Its own
  // reads and stream retain server authorization; refusal removes the controls.
  const personalReady =
    personal?.client === client &&
    personal?.agentId === agentId &&
    result.state.status !== 'error' &&
    (result.state.status !== 'ready' || result.state.value.detail.access !== 'shared');
  return { ...result, personalReady };
}
