import { useEffect, useRef, useState } from 'react';
import { Alert } from '../ui/Alert';
import type { CmaWebClient } from '../api/client';
import { useConnectionGate } from '../connections/useConnectionGate';
import { Button } from '../ui/Button';
import { lifecycleScopeForAgent, useLifecycleIntent } from './lifecycleIntent';
import { lifecycleFailureMessage } from './lifecycleFailure';
import { AgentProvisioningProgress } from './AgentProvisioningProgress';
import './AgentLifecycleCard.css';
import './AgentLifecycleFollowup.css';

const retrySetupStatus = { label: 'Retry setup status', kind: 'status' } as const;

interface Props {
  readonly client: CmaWebClient;
  readonly viewerId: string;
  readonly agentId: string;
}

export function AgentLifecycleFollowup(props: Props) {
  const { client, viewerId, agentId } = props;
  const [scope, setScope] = useState({ client, viewerId, agentId, generation: 0 });
  if (scope.client !== client || scope.viewerId !== viewerId || scope.agentId !== agentId)
    setScope({ client, viewerId, agentId, generation: scope.generation + 1 });
  return <FollowupScope key={scope.generation} {...props} />;
}

function FollowupScope({ client, viewerId, agentId }: Props) {
  const [scope] = useState(() => lifecycleScopeForAgent(viewerId, agentId));
  return scope === undefined ? null : (
    <RetainedSetup client={client} viewerId={viewerId} agentId={agentId} {...scope} />
  );
}

function RetainedSetup({
  client,
  viewerId,
  agentId,
  workspaceId,
  sandboxId,
}: Props & {
  readonly workspaceId?: string;
  readonly sandboxId?: string;
}) {
  const lifecycle = useLifecycleIntent(client, viewerId, workspaceId, sandboxId);
  const connections = useConnectionGate();
  const cleared = useRef(false);
  const { operation, intent, clear } = lifecycle;
  const matches =
    intent?.agentId === agentId &&
    operation?.id === intent.receipt?.id &&
    operation?.agent_id === agentId;
  useEffect(() => {
    if (matches && operation?.status === 'succeeded' && !cleared.current) {
      cleared.current = true;
      clear();
    }
  }, [clear, matches, operation?.status]);
  if (!intent) return null;
  const waiting = matches && operation?.status === 'waiting_for_connection';
  const failed = matches && operation?.status === 'failed';
  const error = lifecycleFailureMessage(lifecycle.failure, retrySetupStatus) ?? connections.error;
  return (
    <>
      {matches && intent.receipt ? (
        <div className="agent-lifecycle-followup__progress">
          <AgentProvisioningProgress receipt={intent.receipt} operation={operation} />
        </div>
      ) : null}
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
}
