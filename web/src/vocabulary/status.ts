import type { AgentStatus, SandboxStatus, WorkspaceStatus, WorktreeStatus } from '../api/types';

const words = {
  provisioning: 'Starting',
  installing: 'Starting',
  active: 'Live',
  pausing: 'Pausing',
  paused: 'Paused',
  reviving: 'Resuming',
  credential_rotating: 'Updating access',
  rotating: 'Resuming',
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
  revived: 'Sandbox resumed',
  rotated: 'Sandbox updated',
  failed: 'Sandbox needs attention',
};

export function eventLabel(type: string): string {
  return events[type] ?? 'Status updated';
}
