import { useEffect, useRef, useState, type ReactNode } from 'react';
import { Alert } from '../ui/Alert';
import type { CmaWebClient } from '../api/client';
import { useConnectionGate } from '../connections/useConnectionGate';
import { Button } from '../ui/Button';
import { lifecycleScopeForAgent, useLifecycleIntent } from './lifecycleIntent';
import { lifecycleFailureMessage } from './lifecycleFailure';
import { AgentProvisioningProgress } from './AgentProvisioningProgress';
import { RepositoryPreparationHelp } from './RepositoryPreparationHelp';
import './AgentLifecycleCard.css';
import './AgentLifecycleFollowup.css';

const retrySetupStatus = { label: 'Retry setup status', kind: 'status' } as const;

export interface LifecycleFollowupView {
  readonly lifecycle?: ReturnType<typeof useLifecycleIntent>;
  readonly context?: { readonly workspaceId?: string; readonly sandboxId?: string };
  readonly feedback: ReactNode;
  readonly details?: ReactNode;
}

interface Props {
  readonly client: CmaWebClient;
  readonly viewerId: string;
  readonly agentId: string;
  readonly enabled?: boolean;
  readonly render?: (view: LifecycleFollowupView) => ReactNode;
}

export function AgentLifecycleFollowup(props: Props) {
  const { client, viewerId, agentId } = props;
  const [scope, setScope] = useState({ client, viewerId, agentId, generation: 0 });
  if (scope.client !== client || scope.viewerId !== viewerId || scope.agentId !== agentId)
    setScope({ client, viewerId, agentId, generation: scope.generation + 1 });
  return <FollowupScope key={scope.generation} {...props} />;
}

function FollowupScope({ client, viewerId, agentId, render, enabled }: Props) {
  const [scope] = useState(() => lifecycleScopeForAgent(viewerId, agentId));
  return scope === undefined ? (
    render?.({ feedback: null })
  ) : (
    <RetainedSetup
      client={client}
      viewerId={viewerId}
      agentId={agentId}
      render={render}
      enabled={enabled}
      {...scope}
    />
  );
}

function RetainedSetup({
  client,
  viewerId,
  agentId,
  workspaceId,
  sandboxId,
  operationId,
  render,
  enabled = true,
}: Props & {
  readonly workspaceId?: string;
  readonly sandboxId?: string;
  readonly operationId: string;
}) {
  const lifecycle = useLifecycleIntent(
    client,
    viewerId,
    workspaceId,
    sandboxId,
    { agentId, operationId },
    enabled,
  );
  const connections = useConnectionGate();
  const cleared = useRef(false);
  const { operation, intent, clear } = lifecycle;
  const matches =
    intent?.agentId === agentId &&
    operation?.id === intent.receipt?.id &&
    operation?.agent_id === agentId;
  useEffect(() => {
    if (enabled && !render && matches && operation?.status === 'succeeded' && !cleared.current) {
      cleared.current = true;
      clear();
    }
  }, [clear, enabled, matches, operation?.status, render]);
  if (!enabled) return render?.({ lifecycle, context: { workspaceId, sandboxId }, feedback: null });
  if (!intent) {
    const feedback = lifecycle.failure ? (
      <Alert tone="error">{lifecycleFailureMessage(lifecycle.failure, retrySetupStatus)}</Alert>
    ) : null;
    return render ? render({ lifecycle, context: { workspaceId, sandboxId }, feedback }) : feedback;
  }
  const waiting = matches && operation?.status === 'waiting_for_connection';
  const failed = matches && operation?.status === 'failed';
  const error = lifecycleFailureMessage(lifecycle.failure, retrySetupStatus) ?? connections.error;
  const details =
    matches && intent.receipt ? (
      <AgentProvisioningProgress receipt={intent.receipt} operation={operation} />
    ) : null;
  const feedback = (
    <>
      {connections.dialog}
      {waiting ? (
        <Alert
          tone="warning"
          actions={
            <Button
              size="sm"
              disabled={lifecycle.pending || connections.pending || !operation?.connection}
              onClick={() => {
                if (operation?.connection)
                  void connections.checkAndRun(
                    operation.connection.target,
                    viewerId,
                    () => void lifecycle.submit(),
                  );
              }}
            >
              Continue Agent setup
            </Button>
          }
        >
          Agent setup needs a connection check to continue.
        </Alert>
      ) : null}
      {failed ? (
        <Alert tone="error">
          {operation.error ?? 'Agent setup could not finish. Your conversation is still available.'}
        </Alert>
      ) : null}
      <RepositoryPreparationHelp
        client={client}
        viewerId={viewerId}
        intent={intent}
        operation={operation}
      />
      {error ? (
        <Alert
          tone="error"
          actions={
            <Button size="sm" onClick={lifecycle.retryStatus}>
              {retrySetupStatus.label}
            </Button>
          }
        >
          {error}
        </Alert>
      ) : null}
    </>
  );
  return render ? (
    render({ lifecycle, context: { workspaceId, sandboxId }, feedback, details })
  ) : (
    <>
      <div className="agent-lifecycle-followup__progress">{details}</div>
      {feedback}
    </>
  );
}
