import type { CmaWebClient } from './api/client';
import type { AgentEvent, Failure, SandboxEvent } from './api/types';
import type { TreeAction } from './shell/Tree';
import type { ProvisioningStep } from './sandbox/progress';
import { readSandboxHistory } from './sandbox/eventHistory';

export type DeleteAction = Extract<
  TreeAction,
  { kind: 'delete_sandbox' | 'remove_workspace' | 'delete_agent' }
>;
export interface DeletionRequest {
  readonly action: DeleteAction;
  readonly key: string;
  readonly accepted: boolean;
}
export interface DeletionObservation {
  readonly name: string;
  readonly complete: boolean;
  readonly blocked?: string;
  readonly failure?: Failure;
  readonly steps: readonly ProvisioningStep[];
  readonly events: readonly SandboxEvent[];
  readonly agentEvents?: readonly AgentEvent[];
  readonly observedAt: string;
}

export function deletionTarget(action: DeleteAction): string {
  return action.kind === 'delete_sandbox'
    ? 'Sandbox'
    : action.kind === 'remove_workspace'
      ? 'Workspace'
      : 'Agent';
}

export { deletionStorageKey, restoreDeletion } from './deletionRecovery';

export async function requestDeletion(client: CmaWebClient, request: DeletionRequest) {
  const options = { idempotencyKey: request.key };
  const action = request.action;
  if (action.kind === 'delete_sandbox') await client.deleteSandbox(action.sandboxId, options);
  else if (action.kind === 'remove_workspace')
    await client.removeWorkspace(action.workspaceId, options);
  else await client.deleteAgent(action.agentId, options);
}

function step(label: string, done: boolean, started: boolean): ProvisioningStep {
  return { label, status: done ? 'Done' : started ? 'In progress' : 'Waiting' };
}

export async function observeDeletion(
  client: CmaWebClient,
  action: DeleteAction,
  signal: AbortSignal,
  accepted: boolean,
  previous: readonly SandboxEvent[] = [],
  previousAgentEvents: readonly AgentEvent[] = [],
): Promise<DeletionObservation> {
  if (action.kind === 'delete_sandbox') {
    const sandbox = await client.readSandbox(action.sandboxId, { signal });
    if (sandbox.id !== action.sandboxId)
      throw new Error('Deletion status referred to another Sandbox.');
    const complete = sandbox.status === 'retired';
    const events =
      accepted || complete ? await readSandboxHistory(client, sandbox.id, signal, previous) : [];
    const completed = new Set(
      events
        .filter((event) => event.kind.type === 'work_step_completed')
        .map((event) => (event.kind.step as { kind?: string } | undefined)?.kind),
    );
    const stopped = complete || completed.has('stop_agents');
    const revoked = completed.has('revoke_retirement_credentials');
    const removed = complete || completed.has('teardown_runtime');
    const latestFailure = [...events]
      .reverse()
      .find(
        (event) =>
          event.kind.type === 'work_step_failed' &&
          ['stop_agents', 'revoke_retirement_credentials', 'teardown_runtime'].includes(
            String((event.kind.step as { kind?: string } | undefined)?.kind),
          ),
      );
    const failureStep = (latestFailure?.kind.step as { kind?: string } | undefined)?.kind;
    const failure =
      latestFailure !== undefined && !complete && !completed.has(failureStep)
        ? (latestFailure.kind.failure as Failure)
        : undefined;
    return {
      name: sandbox.name,
      observedAt: new Date().toISOString(),
      complete,
      events,
      failure,
      blocked:
        sandbox.lifecycle?.checkpoint !== undefined || sandbox.lifecycle?.operation !== undefined
          ? 'Wait for the current Sandbox operation to finish before deleting it.'
          : undefined,
      steps: [
        step('Stop Agents', stopped, accepted),
        step('Remove Sandbox runtime', removed, revoked),
        step('Deletion confirmed', complete, removed),
      ],
    };
  }
  if (action.kind === 'remove_workspace') {
    const workspace = await client.readWorkspace(action.workspaceId, { signal });
    if (workspace.id !== action.workspaceId)
      throw new Error('Deletion status referred to another Workspace.');
    const sandbox = await client.readSandbox(workspace.sandboxId, { signal });
    if (sandbox.id !== workspace.sandboxId)
      throw new Error('Deletion status referred to another Sandbox.');
    const complete = workspace.status === 'removed';
    const live = workspace.agents.some(
      (agent) => !['stopped', 'failed', 'deleted'].includes(agent.status),
    );
    const agentsRemoved = complete || workspace.agents.every((agent) => agent.status === 'deleted');
    const events =
      accepted || complete ? await readSandboxHistory(client, sandbox.id, signal, previous) : [];
    return {
      name: workspace.name,
      observedAt: new Date().toISOString(),
      complete,
      events,
      blocked:
        sandbox.status === 'paused'
          ? 'Resume the Sandbox before deleting this Workspace.'
          : sandbox.lifecycle?.checkpoint !== undefined
            ? 'Wait for the Sandbox to finish saving.'
            : undefined,
      failure: workspace.status === 'failed' ? workspace.failure : undefined,
      steps: [
        step('Stop Agents', complete || !live, accepted || workspace.status === 'removing'),
        step('Remove Agents and Worktrees', agentsRemoved, !live),
        step('Remove Workspace files', complete, agentsRemoved && workspace.status === 'removing'),
        step('Deletion confirmed', complete, complete),
      ],
    };
  }
  const agent = await client.readAgent(action.agentId, { signal });
  if (agent.id !== action.agentId || agent.worktree.agentId !== agent.id)
    throw new Error('Deletion status referred to another Agent or Worktree.');
  const complete = agent.status === 'deleted' && agent.worktree.status === 'removed';
  const agentEvents = [...previousAgentEvents];
  if (accepted || complete) {
    let cursor = agentEvents.at(-1)?.seq ?? 0;
    for (;;) {
      const page = await client.readAgentEvents(agent.id, { cursor, signal });
      for (const event of page.items) {
        if (!Number.isSafeInteger(event.seq) || event.seq <= cursor)
          throw new Error('Agent deletion events did not advance.');
        agentEvents.push(event);
        cursor = event.seq;
      }
      if (page.next === undefined) break;
      if (page.next !== cursor || page.items.length === 0)
        throw new Error('Agent deletion event cursor did not advance.');
    }
  }
  const completed = new Set(
    agentEvents
      .filter((event) => event.kind.type === 'work_step_completed')
      .map((event) => event.kind.step),
  );
  const closed = complete || completed.has('ensure_process_closed');
  const homeRemoved = complete || completed.has('remove_home');
  const latestFailure = [...agentEvents]
    .reverse()
    .find(
      (event) =>
        event.kind.type === 'work_step_failed' &&
        ['ensure_process_closed', 'remove_home', 'remove_worktree', 'publish_deleted'].includes(
          String(event.kind.step),
        ),
    );
  return {
    name: agent.title?.trim() || 'Untitled agent',
    observedAt: new Date().toISOString(),
    complete,
    events: [],
    agentEvents,
    failure:
      latestFailure !== undefined && !complete && !completed.has(latestFailure.kind.step)
        ? (latestFailure.kind.failure as Failure)
        : undefined,
    blocked: !['stopped', 'failed', 'deleted'].includes(agent.status)
      ? 'Stop this Agent before deleting it and its Worktree.'
      : agent.sandbox.status === 'paused'
        ? 'Resume the Sandbox before deleting this Agent.'
        : undefined,
    steps: [
      step('Confirm Agent stopped', closed, accepted),
      step('Remove Agent files', homeRemoved, closed),
      step('Remove Worktree', agent.worktree.status === 'removed', homeRemoved),
      step('Deletion confirmed', complete, agent.worktree.status === 'removed'),
    ],
  };
}
