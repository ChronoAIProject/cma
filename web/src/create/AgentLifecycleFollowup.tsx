import { useEffect, useRef, useState } from 'react';
import type { CmaWebClient } from '../api/client';
import { useConnectionGate } from '../connections/useConnectionGate';
import { Button } from '../ui/Button';
import { lifecycleScopeForAgent, useLifecycleIntent } from './lifecycleIntent';

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
  const error = lifecycle.failure ?? connections.error;
  return (
    <>
      {connections.dialog}
      {waiting ? (
        <div className="agent-action-error" role="status">
          <span>Agent setup needs a connection check to continue.</span>
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
        </div>
      ) : null}
      {failed ? (
        <p className="agent-action-error" role="alert">
          {operation.error ?? 'Agent setup could not finish. Your conversation is still available.'}
        </p>
      ) : null}
      {error ? (
        <div className="agent-action-error" role="alert">
          <span>{error}</span>
          <Button size="sm" onClick={lifecycle.retryStatus}>
            Retry setup status
          </Button>
        </div>
      ) : null}
    </>
  );
}
