import { useMemo } from 'react';
import { ApiError, type EnvironmentFeature } from '@chronoai/cma-sdk';
import type { AgentFilesClient } from '../../api/agentFiles';
import { useAuthorizedRead } from '../../resources/useAuthorizedRead';

export function useAgentEnvironment(
  client: AgentFilesClient | undefined,
  agentId: string,
  active: boolean,
) {
  const read = useMemo(
    () =>
      !active || !client?.environment
        ? undefined
        : async (signal: AbortSignal) => {
            try {
              const value = await client.environment!(agentId, { signal });
              signal.throwIfAborted();
              if (value.agent_id !== agentId)
                throw new Error('Agent environment identity changed.');
              return { value, revalidateAt: value.revalidate_at };
            } catch (error) {
              if (error instanceof ApiError && error.status === 404) return { value: undefined };
              throw error;
            }
          },
    [client, agentId, active],
  );
  return useAuthorizedRead(read);
}

export function FeatureAvailability({
  name,
  feature,
}: {
  readonly name: string;
  readonly feature: EnvironmentFeature;
}) {
  if (name === 'Browser' && feature.reason === 'browser_paused')
    return <p role="status">Browser observation paused.</p>;
  if (name === 'Browser' && feature.reason === 'browser_disconnected')
    return <p role="status">Browser disconnected.</p>;
  const description = {
    available: `${name} is available but cannot be displayed here.`,
    unsupported: `${name} is not supported by this environment.`,
    not_enabled: `${name} is not enabled for this environment.`,
    forbidden: `${name} access is not permitted.`,
    temporarily_unavailable: `${name} is temporarily unavailable. Try again.`,
  }[feature.state];
  return <p role="status">{description}</p>;
}
