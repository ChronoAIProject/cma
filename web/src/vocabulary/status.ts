import type { AgentStatus, SandboxStatus, WorkspaceStatus, WorktreeStatus } from '../api/types';

const words = {
  provisioning: 'Provisioning',
  installing: 'Installing',
  active: 'Live',
  pausing: 'Pausing',
  paused: 'Paused',
  reviving: 'Resuming',
  credential_rotating: 'Updating access',
  rotating: 'Updating runtime',
  failed: 'Needs attention',
  retiring: 'Deleting',
  retired: 'Deleted',
  preparing: 'Starting',
  ready: 'Ready',
  removing: 'Deleting',
  removed: 'Deleted',
  creating: 'Starting',
  starting: 'Starting',
  working: 'Working',
  waiting_approval: 'Needs approval',
  stopped: 'Stopped',
  deleted: 'Deleted',
} as const satisfies Record<AgentStatus | SandboxStatus | WorkspaceStatus | WorktreeStatus, string>;

export type ResourceStatusLabel = (typeof words)[keyof typeof words];

export type StepStatusLabel =
  'Waiting' | 'In progress' | 'Done' | 'Needs you' | 'Paused' | "Didn't work";

/**
 * How a status is drawn — the shared style contracts
 *
 * - `active`: something is happening right now (an accent dot with a ping).
 * - `neutral`: settled and usable (a quiet static dot).
 * - `warning`: waiting for a person.
 * - `error`: needs attention.
 * - `muted`: ended or held; no dot, a quieter word.
 *
 * One mapping for every surface, so a word never changes colour between the
 * sidebar and the page it opens.
 */
export type StatusTone = 'active' | 'neutral' | 'warning' | 'error' | 'muted';

const tones = {
  provisioning: 'active',
  installing: 'active',
  active: 'neutral',
  pausing: 'active',
  paused: 'muted',
  reviving: 'active',
  credential_rotating: 'active',
  rotating: 'active',
  failed: 'error',
  retiring: 'active',
  retired: 'muted',
  preparing: 'active',
  ready: 'neutral',
  removing: 'active',
  removed: 'muted',
  creating: 'active',
  starting: 'active',
  working: 'active',
  waiting_approval: 'warning',
  stopped: 'muted',
  deleted: 'muted',
} as const satisfies Record<keyof typeof words, StatusTone>;

export function statusTone(
  status: AgentStatus | SandboxStatus | WorkspaceStatus | WorktreeStatus,
): StatusTone {
  return tones[status];
}

export function statusLabel(
  status: AgentStatus | SandboxStatus | WorkspaceStatus | WorktreeStatus,
): ResourceStatusLabel {
  return words[status];
}

const events: Readonly<Record<string, string>> = {
  created: 'Sandbox created',
  work_step_completed: 'Preparation step completed',
  work_step_failed: 'Preparation needs attention',
  status_changed: 'Sandbox status changed',
  workspace_created: 'Workspace created',
  workspace_ready: 'Workspace ready',
  workspace_failed: 'Workspace needs attention',
  workspace_removed: 'Workspace removed',
  worktree_created: 'Worktree created',
  worktree_removed: 'Worktree removed',
  agent_created: 'Agent created',
  agent_status_changed: 'Agent status changed',
  agent_deleted: 'Agent deleted',
  credential_changed: 'Account access updated',
  checkpoint_sealed: 'Progress saved',
  runtime_recovery_admitted: 'Runtime recovery requested',
  runtime_recovery_quiesced: 'Runtime recovery prepared',
  revived: 'Sandbox resumed',
  home_restored: 'Sandbox home restored',
  rotated: 'Sandbox updated',
  failed: 'Sandbox needs attention',
};

export function eventLabel(type: string): string {
  return events[type] ?? 'Status updated';
}
