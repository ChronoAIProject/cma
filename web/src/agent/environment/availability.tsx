import { useMemo } from 'react';
import { ApiError, type EnvironmentFeature } from '@cma/client';
import type { AgentFilesClient } from '../../api/agentFiles';
import { useAuthorizedRead } from '../../resources/useAuthorizedRead';
import type { useAgentAccess } from '../../resources/agentAccess';
import { Button } from '../../ui/Button';

export function useAgentEnvironment(
  client: AgentFilesClient | undefined,
  agentId: string,
  active: boolean,
  subject?: string,
) {
  const scope = useMemo(() => ({ client, agentId, subject }), [client, agentId, subject]);
  const read = useMemo(
    () =>
      !active || !scope.client?.environment
        ? undefined
        : async (signal: AbortSignal) => {
            try {
              const value = await scope.client!.environment!(scope.agentId, { signal });
              signal.throwIfAborted();
              if (value.agent_id !== scope.agentId)
                throw new Error('Agent environment identity changed.');
              return { value, revalidateAt: value.revalidate_at };
            } catch (error) {
              if (error instanceof ApiError && error.status === 404) return { value: undefined };
              throw error;
            }
          },
    [scope, active],
  );
  return useAuthorizedRead(read);
}

export function EnvironmentAvailability({
  environment: { state, reload },
}: {
  readonly environment: ReturnType<typeof useAgentEnvironment>;
}) {
  return (
    <>
      <p role="status">
        {state.status === 'ready'
          ? 'Environment capabilities not reported.'
          : state.status === 'loading'
            ? 'Checking environment capabilities…'
            : state.transient
              ? 'Environment capabilities are temporarily unavailable. Try again.'
              : 'Environment capabilities are unavailable. Try again.'}
      </p>
      {state.status === 'error' ? (
        <Button size="sm" onClick={reload}>
          Check environment again
        </Button>
      ) : null}
    </>
  );
}

export function FileAvailability({
  shared,
  access,
  environment,
  reported,
}: {
  readonly shared: boolean;
  readonly access: ReturnType<typeof useAgentAccess>;
  readonly environment: ReturnType<typeof useAgentEnvironment>;
  readonly reported: boolean;
}) {
  if (
    shared &&
    (access.state.status !== 'ready' ||
      !access.state.value.access?.capabilities.includes('view_files'))
  )
    return (
      <>
        <p role="status">
          {access.state.status === 'loading'
            ? 'Checking file access…'
            : access.state.status === 'error'
              ? access.state.message
              : 'File access is unavailable for this Agent.'}
        </p>
        <Button size="sm" onClick={access.reload}>
          Check access again
        </Button>
      </>
    );
  if (reported && environment.state.status !== 'ready')
    return <EnvironmentAvailability environment={environment} />;
  const feature = environment.state.status === 'ready' ? environment.state.value?.files : undefined;
  if (feature && feature.state !== 'available')
    return <FeatureAvailability name="Files" feature={feature} />;
  return <p role="status">File access is unavailable for this Agent.</p>;
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
