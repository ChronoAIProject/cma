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
  viewerId?: string,
  context?: object,
) {
  const [personal, setPersonal] = useState<{
    readonly client: ResourceViewsClient;
    readonly agentId: string;
    readonly viewerId?: string;
    readonly context?: object;
    readonly lineage: {
      readonly agentId: string;
      readonly workspaceId: string;
      readonly sandboxId: string;
    };
  }>();
  const read = useMemo(
    () =>
      client === undefined || agentId === undefined
        ? undefined
        : async (signal: AbortSignal): Promise<AuthorizedValue<AgentAccessValue>> => {
            const detail = await client.agents.readAccessible(agentId, { signal });
            signal.throwIfAborted();
            if (detail.id !== agentId) throw new Error('Agent access identity changed.');
            if (detail.access !== 'shared') {
              setPersonal({
                client,
                agentId,
                viewerId,
                context,
                lineage: {
                  agentId: detail.id,
                  workspaceId: detail.workspace.id,
                  sandboxId: detail.sandbox.id,
                },
              });
              return { value: { detail } };
            }
            setPersonal(undefined);
            const access = await client.resourceAccess.effective('agent', agentId, { signal });
            if (!access.capabilities.includes('view_details'))
              throw new Error('Agent access is unavailable.');
            return { value: { detail, access }, revalidateAt: access.revalidate_at };
          },
    [client, agentId, viewerId, context],
  );
  const result = useAuthorizedRead(read);
  // Only transient uncertainty can retain an already authorized personal view.
  // Refusal discards the classification permanently, including during later retries.
  if (personal !== undefined && result.state.status === 'error' && !result.state.transient)
    setPersonal(undefined);
  const personalReady =
    personal !== undefined &&
    personal?.client === client &&
    personal?.agentId === agentId &&
    personal?.viewerId === viewerId &&
    personal?.context === context &&
    (result.state.status !== 'error' || result.state.transient === true) &&
    (result.state.status !== 'ready' || result.state.value.detail.access !== 'shared');
  return {
    ...result,
    personalReady,
    personalLineage:
      personalReady && result.state.status === 'ready' ? personal.lineage : undefined,
    personalReadOnly: personalReady && result.state.status !== 'ready',
  };
}
