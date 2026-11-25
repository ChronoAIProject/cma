import { useEffect, useMemo, useState } from 'react';
import type { AgentLifecycleOperationView, CmaWebClient } from '../api/client';
import type { ConnectionTarget } from '../connections/client';
import { useConnectionGate } from '../connections/useConnectionGate';
import { routePath } from '../routing/routes';
import { Alert } from '../ui/Alert';
import { Button } from '../ui/Button';
import { orderedProvisioningEvents } from './agentProvisioning';
import type { useLifecycleIntent } from './lifecycleIntent';

type Intent = NonNullable<ReturnType<typeof useLifecycleIntent>['intent']>;
interface Props {
  readonly client: CmaWebClient;
  readonly viewerId?: string;
  readonly intent?: Intent;
  readonly operation?: AgentLifecycleOperationView;
}

export function RepositoryPreparationHelp({ client, viewerId, intent, operation }: Props) {
  if (
    !viewerId ||
    !intent?.body.repository ||
    !intent.receipt ||
    operation?.id !== intent.receipt.id ||
    operation.status !== 'failed' ||
    (intent.agentId !== undefined && intent.agentId !== operation.agent_id)
  )
    return null;
  const workspace = orderedProvisioningEvents(operation)
    .filter((event) => event.phase === 'workspace_runtime')
    .at(-1);
  if (operation.phase !== 'workspace_runtime' && workspace?.status !== 'failed') return null;
  return (
    <PreparationCheck
      key={JSON.stringify([
        viewerId,
        intent.key,
        operation.id,
        operation.agent_id,
        intent.body.workspace_id,
        intent.body.sandbox_id,
      ])}
      client={client}
      viewerId={viewerId}
      intent={intent}
      operation={operation}
    />
  );
}

type Target = { readonly connection: ConnectionTarget; readonly sandboxId: string };

async function verifyTarget(
  client: CmaWebClient,
  viewerId: string,
  intent: Intent,
  operation: AgentLifecycleOperationView,
  signal: AbortSignal,
): Promise<Target | undefined> {
  let sandboxId: string;
  let connection: ConnectionTarget;
  if (operation.agent_id) {
    const agent = await client.readAgent(operation.agent_id, { signal });
    if (
      signal.aborted ||
      agent.id !== operation.agent_id ||
      agent.worktree.agentId !== agent.id ||
      agent.worktree.workspaceId !== agent.workspace.id ||
      agent.worktree.sandboxId !== agent.sandbox.id ||
      (intent.body.workspace_id !== undefined && agent.workspace.id !== intent.body.workspace_id)
    )
      return undefined;
    sandboxId = agent.sandbox.id;
    connection = { type: 'agent', id: agent.id };
  } else if (intent.body.workspace_id) {
    const workspace = await client.readWorkspace(intent.body.workspace_id, { signal });
    if (signal.aborted || workspace.id !== intent.body.workspace_id) return undefined;
    sandboxId = workspace.sandboxId;
    connection = { type: 'workspace', id: workspace.id };
  } else return undefined;
  if (intent.body.sandbox_id !== undefined && sandboxId !== intent.body.sandbox_id)
    return undefined;
  const sandbox = await client.readSandbox(sandboxId, { signal });
  return !signal.aborted && sandbox.id === sandboxId && sandbox.owner === viewerId
    ? { connection, sandboxId }
    : undefined;
}

function PreparationCheck({ client, viewerId, intent, operation }: Required<Props>) {
  const scope = useMemo(() => ({ client }), [client]);
  const [attempt, setAttempt] = useState(0);
  const [verified, setVerified] = useState<{
    readonly scope: object;
    readonly attempt: number;
    readonly target?: Target;
  }>();
  const [confirmed, setConfirmed] = useState<object>();
  const current = verified?.scope === scope && verified.attempt === attempt;
  const target = current ? verified.target : undefined;
  const connections = useConnectionGate(
    target !== undefined,
    scope,
    'Connect the accounts this repository needs. CMA verifies the connections; this check does not retry repository preparation or send your task.',
    true,
  );
  useEffect(() => {
    const abort = new AbortController();
    const deadline = setTimeout(() => {
      abort.abort();
      setVerified({ scope, attempt });
    }, 5000);
    void verifyTarget(client, viewerId, intent, operation, abort.signal).then(
      (target) => {
        clearTimeout(deadline);
        if (!abort.signal.aborted) setVerified({ scope, attempt, target });
      },
      () => {
        clearTimeout(deadline);
        if (!abort.signal.aborted) setVerified({ scope, attempt });
      },
    );
    return () => {
      clearTimeout(deadline);
      abort.abort();
    };
  }, [attempt, client, intent, operation, scope, viewerId]);
  const bound = Boolean(operation.agent_id || intent.body.workspace_id);
  return (
    <section aria-label="Repository preparation help">
      {connections.dialog}
      <Alert
        tone="warning"
        title="Repository preparation needs attention"
        actions={
          <Button
            size="sm"
            disabled={!target || connections.pending}
            onClick={() => {
              if (!target) return;
              setConfirmed(undefined);
              void connections.checkAndRun(target.connection, viewerId, () => setConfirmed(scope));
            }}
          >
            {connections.pending
              ? 'Checking repository connection…'
              : 'Check repository connection'}
          </Button>
        }
      >
        <p>
          A connection check may help identify the next step. Your original task is saved. Checking
          connections does not retry the failed preparation or send the task.
        </p>
        {!bound ? (
          <p>The failed operation has not confirmed an Agent or Workspace to check.</p>
        ) : !current ? (
          <p>Verifying the resource for your current account…</p>
        ) : !target ? (
          <>
            <p>The resource for this check could not be verified for your current account.</p>
            <Button size="sm" onClick={() => setAttempt((value) => value + 1)}>
              Retry resource check
            </Button>
          </>
        ) : null}
        {connections.error ? (
          <p role="alert">The connection check could not be confirmed. Try checking again.</p>
        ) : confirmed === scope ? (
          <p role="status">
            Connections confirmed. The original repository preparation failure has not been
            recovered. Your task has not been retried.
          </p>
        ) : null}
        <p>
          The failed setup may still use Sandbox resources.{' '}
          {target ? (
            <a href={routePath({ kind: 'sandbox', sandboxId: target.sandboxId })}>
              Manage this Sandbox
            </a>
          ) : (
            'Open Sandbox management'
          )}{' '}
          to review them before deciding whether to retire the environment.
        </p>
      </Alert>
    </section>
  );
}
