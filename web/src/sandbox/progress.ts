import type { AgentDetail, SandboxDetail, SandboxEvent, WorkspaceDetail } from '../api/types';
import type { HumanStatus } from '../vocabulary/status';

export interface ProvisioningStep {
  readonly label: string;
  readonly status: HumanStatus;
  readonly detail?: string;
}

function record(value: unknown): Record<string, unknown> {
  return typeof value === 'object' && value !== null ? (value as Record<string, unknown>) : {};
}

const sandboxSteps = [
  'Starting the sandbox',
  'Installing software',
  'Connecting services',
  'Preparing workspaces',
  'Ready',
];
const workSteps: Readonly<Record<string, number>> = {
  provision_runtime: 0,
  run_install: 1,
  mint_initial_credential: 2,
  deliver_initial_credential: 2,
  create_profile_workspace: 3,
  activate: 4,
};

/** No timer advances a step: only the public snapshot and recorded events do. */
export function sandboxProgress(
  sandbox: SandboxDetail,
  events: readonly SandboxEvent[],
): ProvisioningStep[] {
  const own = events.filter((event) => event.sandboxId === sandbox.id);
  const complete = new Set(
    own
      .filter((event) => event.kind.type === 'work_step_completed')
      .map((event) => record(event.kind.step).kind),
  );
  const active =
    sandbox.status === 'active' ||
    own.some((event) => event.kind.type === 'status_changed' && event.kind.to === 'active');
  const completed = [
    active || complete.has('provision_runtime') || sandbox.status === 'installing',
    active || complete.has('run_install'),
    active || complete.has('deliver_initial_credential'),
    active,
    active,
  ];
  const latestFailure = [...own].reverse().find((event) => event.kind.type === 'work_step_failed');
  const failedStep =
    latestFailure === undefined
      ? undefined
      : workSteps[String(record(latestFailure.kind.step).kind)];
  const current = completed.findIndex((value) => !value);
  return sandboxSteps.map((label, index) => ({
    label,
    status: completed[index]
      ? 'Done'
      : sandbox.status === 'failed' && index === (failedStep ?? current)
        ? "Didn't work"
        : index === current
          ? sandbox.status === 'paused'
            ? 'Paused'
            : 'In progress'
          : 'Waiting',
    detail:
      sandbox.status === 'failed' && index === (failedStep ?? current)
        ? sandbox.failure?.detail
        : undefined,
  }));
}

export function workspaceProgress(
  workspace: WorkspaceDetail,
  events: readonly SandboxEvent[],
): ProvisioningStep[] {
  const ready =
    workspace.status === 'ready' ||
    events.some(
      (event) =>
        event.sandboxId === workspace.sandboxId &&
        event.kind.type === 'workspace_ready' &&
        event.kind.workspace_id === workspace.id,
    );
  const prepared = ready || workspace.resolvedCommit !== undefined;
  const failed = workspace.status === 'failed';
  return [
    {
      label:
        workspace.source.type === 'git_repository' ? 'Cloning repository' : 'Creating directory',
      status: prepared ? 'Done' : failed ? "Didn't work" : 'In progress',
    },
    {
      label: 'Running setup',
      status: ready ? 'Done' : prepared ? (failed ? "Didn't work" : 'In progress') : 'Waiting',
    },
    { label: 'Ready', status: ready ? 'Done' : 'Waiting' },
  ];
}

export function agentProgress(
  agent: AgentDetail,
  events: readonly SandboxEvent[],
): ProvisioningStep[] {
  const ready =
    agent.readyAt !== undefined ||
    ['ready', 'working', 'waiting_approval', 'paused', 'stopped'].includes(agent.status) ||
    events.some(
      (event) =>
        event.sandboxId === agent.sandbox.id &&
        event.kind.type === 'agent_status_changed' &&
        event.kind.agent_id === agent.id &&
        event.kind.to === 'ready',
    );
  const worktree = agent.worktree.status === 'ready' || ready;
  return [
    {
      label: 'Creating worktree',
      status: worktree
        ? 'Done'
        : agent.worktree.status === 'failed'
          ? "Didn't work"
          : 'In progress',
    },
    {
      label: 'Preparing the agent',
      status: ready
        ? 'Done'
        : worktree
          ? agent.status === 'failed'
            ? "Didn't work"
            : 'In progress'
          : 'Waiting',
    },
    { label: 'Ready', status: ready ? 'Done' : 'Waiting' },
  ];
}
