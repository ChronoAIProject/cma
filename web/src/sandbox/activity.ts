import type { AgentEvent, SandboxEvent } from '../api/types';
import { eventLabel } from '../vocabulary/status';

export const activityLimit = 100;

export interface ActivityEntry {
  readonly id: string;
  readonly at: string;
  readonly message: string;
}

const steps: Readonly<Record<string, string>> = {
  provision_runtime: 'Start sandbox environment',
  run_install: 'Install software',
  mint_initial_credential: 'Prepare service access',
  deliver_initial_credential: 'Connect services',
  create_profile_workspace: 'Prepare workspace',
  activate: 'Confirm sandbox is ready',
  run_lifecycle_operation: 'Sandbox operation stage',
  stop_agents: 'Stop agents',
  revoke_retirement_credentials: 'Remove service access',
  teardown_runtime: 'Remove sandbox environment',
  mark_retired: 'Confirm deletion',
  ensure_process_closed: 'Stop agent process',
  remove_home: 'Remove agent files',
  remove_worktree: 'Remove worktree',
  publish_deleted: 'Confirm agent deletion',
};
const statuses: Readonly<Record<string, string>> = {
  provisioning: 'Starting',
  installing: 'Starting',
  active: 'Live',
  pausing: 'Pausing',
  paused: 'Paused',
  reviving: 'Resuming',
  rotating: 'Resuming',
  credential_rotating: 'Updating access',
  failed: 'Needs attention',
  retiring: 'Deleting',
  retired: 'Deleted',
  starting: 'Starting',
  ready: 'Ready',
  working: 'Working',
  waiting_approval: 'Needs approval',
  stopped: 'Stopped',
  deleted: 'Deleted',
};
const sandboxKinds = new Set([
  'created',
  'workspace_created',
  'workspace_ready',
  'workspace_failed',
  'workspace_removed',
  'worktree_created',
  'worktree_removed',
  'agent_created',
  'agent_deleted',
  'credential_changed',
  'checkpoint_sealed',
  'revived',
  'rotated',
  'failed',
]);

function known(values: Readonly<Record<string, string>>, key: unknown): string | undefined {
  return typeof key === 'string' && Object.hasOwn(values, key) ? values[key] : undefined;
}

export function activityEntry(
  event: SandboxEvent | AgentEvent,
  source: 'sandbox' | 'agent',
): ActivityEntry | undefined {
  const timestamp = Date.parse(event.appendedAt);
  if (!Number.isFinite(timestamp) || !Number.isSafeInteger(event.seq) || event.seq < 1) return;
  const kind = event.kind;
  let message: string | undefined;
  if (kind.type === 'work_step_completed' || kind.type === 'work_step_failed') {
    const step =
      typeof kind.step === 'object' && kind.step !== null
        ? (kind.step as { kind?: unknown }).kind
        : kind.step;
    const label = known(steps, step);
    if (label !== undefined)
      message = `${label}: ${kind.type === 'work_step_completed' ? 'completed' : 'needs attention'}`;
  } else if (
    kind.type === 'status_changed' ||
    (source === 'sandbox' && kind.type === 'agent_status_changed')
  ) {
    const label = known(statuses, kind.to);
    if (label !== undefined)
      message = `${source === 'agent' || kind.type === 'agent_status_changed' ? 'Agent' : 'Sandbox'}: ${label}`;
  } else if (source === 'sandbox' && sandboxKinds.has(kind.type)) {
    message = eventLabel(kind.type);
  } else if (source === 'agent' && kind.type === 'deleted') {
    message = 'Agent deleted';
  } else if (source === 'agent' && kind.type === 'stopped') {
    message = 'Agent stopped';
  }
  return message === undefined
    ? undefined
    : { id: `${source}-${event.seq}`, at: new Date(timestamp).toISOString(), message };
}

export function activityEntries(
  events: readonly (SandboxEvent | AgentEvent)[],
  source: 'sandbox' | 'agent',
): ActivityEntry[] {
  return events
    .flatMap((event) => {
      const entry = activityEntry(event, source);
      return entry === undefined ? [] : [entry];
    })
    .slice(-activityLimit);
}
