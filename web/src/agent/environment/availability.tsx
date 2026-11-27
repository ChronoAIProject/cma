import { useCallback, useMemo, useRef } from 'react';
import { ApiError, type AgentEnvironment, type EnvironmentFeature } from '@cma/client';
import type { AgentFilesClient } from '../../api/agentFiles';
import { useAuthorizedRead } from '../../resources/useAuthorizedRead';
import type { useAgentAccess } from '../../resources/agentAccess';
import { Button } from '../../ui/Button';
import { ComputerToolsHelp } from './ComputerToolsHelp';

export function useAgentEnvironment(
  client: AgentFilesClient | undefined,
  agentId: string,
  active: boolean,
  subject?: string,
  runtimeVersion?: string,
) {
  const scope = useMemo(
    () => ({ client, agentId, subject, runtimeVersion }),
    [client, agentId, subject, runtimeVersion],
  );
  const observationScope = useMemo(() => ({ scope, active }), [scope, active]);
  const observed = useRef<{ scope: typeof observationScope; value: AgentEnvironment }>(undefined);
  const read = useMemo(() => {
    const { scope: current, active: enabled } = observationScope;
    if (!enabled || !current.client?.environment) return undefined;
    return async (signal: AbortSignal) => {
      try {
        const response = await current.client!.environment!(current.agentId, { signal });
        signal.throwIfAborted();
        if (response.agent_id !== current.agentId)
          throw new Error('Agent environment identity changed.');
        const previous =
          observed.current?.scope === observationScope ? observed.current.value : undefined;
        const value = retainBrowserObservation(response, previous);
        observed.current = { scope: observationScope, value };
        const accessDeadline = value.access_revalidate_at;
        return {
          value,
          revalidateAt: accessDeadline === undefined ? value.revalidate_at : accessDeadline,
          refreshAt: accessDeadline === undefined ? undefined : value.revalidate_at,
        };
      } catch (error) {
        signal.throwIfAborted();
        observed.current = undefined;
        if (error instanceof ApiError && error.status === 404) return { value: undefined };
        throw error;
      }
    };
  }, [observationScope]);
  const { state, reload, refreshing } = useAuthorizedRead(read, false, {
    scope,
    applies: ownerRetryDeadline,
  });
  const reloadEnvironment = useCallback(() => {
    observed.current = undefined;
    reload();
  }, [reload]);
  return { state, reload: reloadEnvironment, refreshing };
}

function retainBrowserObservation(current: AgentEnvironment, previous?: AgentEnvironment) {
  // An observation can be pending on a different replica even with the same
  // current access policy and runtime. Control always uses the current response.
  if (
    current.access_revalidate_at !== undefined &&
    current.browser.state === 'temporarily_unavailable' &&
    current.browser.reason === 'observation_pending' &&
    current.runtime_incarnation !== null &&
    previous?.runtime_incarnation === current.runtime_incarnation &&
    previous.policy_revision === current.policy_revision &&
    (previous.browser.state === 'available' || previous.browser.state === 'not_enabled')
  )
    return { ...current, browser: previous.browser };
  return current;
}

function ownerRetryDeadline(error: ApiError): boolean {
  return (
    error.status === 503 &&
    error.code === 'agent_owner_unavailable' &&
    error.retryAfterMs !== undefined
  );
}

export function EnvironmentAvailability({
  environment: { state, reload },
  subject,
}: {
  readonly subject: 'file' | 'screen' | 'operation';
  readonly environment: ReturnType<typeof useAgentEnvironment>;
}) {
  const pending =
    subject === 'file' ? 'Checking file access…' : `Checking ${subject} availability…`;
  const unavailable =
    subject === 'file'
      ? 'File access is unavailable.'
      : `${subject === 'screen' ? 'Screen' : 'Operation output'} is unavailable.`;
  return (
    <>
      <p role="status">
        {state.status === 'ready'
          ? `${subject === 'file' ? 'File' : subject === 'screen' ? 'Screen' : 'Operation'} availability was not reported.`
          : state.status === 'loading'
            ? pending
            : state.transient
              ? `We couldn’t check ${subject} availability.`
              : unavailable}
      </p>
      {state.status === 'error' ? (
        <Button size="sm" onClick={reload}>
          Try again
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
  onReturnToConversation,
}: {
  readonly onReturnToConversation?: () => void;
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
              ? 'We couldn’t check file access.'
              : 'You do not have access to these files.'}
        </p>
        <Button size="sm" onClick={access.reload}>
          Check access again
        </Button>
      </>
    );
  if (reported && environment.state.status !== 'ready')
    return <EnvironmentAvailability subject="file" environment={environment} />;
  const feature = environment.state.status === 'ready' ? environment.state.value?.files : undefined;
  if (feature && feature.state !== 'available')
    return (
      <FeatureAvailability
        name="Files"
        feature={feature}
        onRetry={environment.reload}
        onReturnToConversation={onReturnToConversation}
      />
    );
  return <p role="status">File access is unavailable for this Agent.</p>;
}

export function FeatureAvailability({
  name,
  feature,
  onRetry,
  policyDisabled = false,
  onReturnToConversation,
}: {
  readonly policyDisabled?: boolean;
  readonly onReturnToConversation?: () => void;
  readonly onRetry?: () => void;
  readonly name: string;
  readonly feature: EnvironmentFeature;
}) {
  if (name === 'Browser' && feature.reason === 'browser_paused')
    return (
      <>
        <p role="status">
          No live screen is available right now. You can continue reading this conversation.
        </p>
        {onRetry ? (
          <Button size="sm" onClick={onRetry}>
            Check again
          </Button>
        ) : null}
      </>
    );
  if (name === 'Browser' && feature.reason === 'browser_disconnected')
    return <p role="status">Browser disconnected.</p>;
  if (feature.state === 'not_enabled' && (name === 'Browser' || name === 'Files'))
    return (
      <div className="agent-computer__guidance">
        <p role="status">
          {name === 'Browser'
            ? 'Live screen is not enabled'
            : 'File browsing is not available for this environment.'}
        </p>
        {name === 'Browser' && policyDisabled ? (
          <p>
            This service does not currently allow you to enable a live screen for this environment.
          </p>
        ) : null}
        {onReturnToConversation ? <p>You can continue the conversation</p> : null}
        <div className="agent-computer__guidance-actions">
          {onReturnToConversation ? (
            <Button size="sm" variant="primary" onClick={onReturnToConversation}>
              Back to conversation
            </Button>
          ) : null}
          <ComputerToolsHelp />
        </div>
      </div>
    );
  const fileDescription = {
    available: 'File browsing is available.',
    unsupported: 'This environment does not support file browsing.',
    not_enabled: 'File browsing is not enabled for this environment.',
    forbidden: 'You do not have access to these files.',
    temporarily_unavailable: 'We couldn’t check file availability.',
  }[feature.state];
  const display = name === 'Browser' ? 'Live screen' : name;
  const description = {
    available: `${display} is available but cannot be displayed here.`,
    unsupported: `${display} is not supported by this environment.`,
    not_enabled: `${display} is not enabled for this environment.`,
    forbidden: `${display} access is not permitted.`,
    temporarily_unavailable: `${display} is temporarily unavailable. Try again.`,
  }[feature.state];
  return (
    <>
      <p role="status">{name === 'Files' ? fileDescription : description}</p>
      {onRetry && ['temporarily_unavailable', 'forbidden'].includes(feature.state) ? (
        <Button size="sm" onClick={onRetry}>
          Try again
        </Button>
      ) : null}
    </>
  );
}
