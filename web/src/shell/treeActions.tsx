import type { AgentStatus, SandboxSummary, WorkspaceDetail } from '../api/types';
import { sandboxChildrenAvailable, sandboxLifecycleAction } from '../sandbox/lifecycle';
import {
  sandboxDeletionReason,
  sandboxHasParkedRuntimeUpdate,
} from '../sandbox/deletionEligibility';
import {
  IconPauseOutline16,
  IconPlayOutline16,
  IconPlusOutline16,
  IconDownloadOutline16,
  IconDataOutline16,
  IconStopFill16,
  IconTrashOutline16,
} from '../ui/icons';
import type { TreePermissions } from './Tree';
import type { TreeRowAction } from './TreeRow';

export function sandboxActions(
  sandbox: SandboxSummary,
  permissions: TreePermissions,
  stale: boolean,
  submitting: boolean,
  lifecycleBusy = false,
): TreeRowAction[] {
  const busy = submitting || lifecycleBusy;
  const actions: TreeRowAction[] = [];
  if (permissions.createAgent)
    actions.push({
      id: 'new_agent',
      label: 'New agent',
      icon: <IconPlusOutline16 />,
      disabled: busy || stale || !sandboxChildrenAvailable(sandbox),
      reason: !sandboxChildrenAvailable(sandbox)
        ? 'Available when the Sandbox is active.'
        : undefined,
    });
  if (permissions.createWorkspace)
    actions.push({
      id: 'new_workspace',
      label: 'New workspace',
      icon: <IconPlusOutline16 />,
      disabled: busy || stale || !sandboxChildrenAvailable(sandbox),
      reason: !sandboxChildrenAvailable(sandbox)
        ? 'Available when the Sandbox is active.'
        : undefined,
    });
  if (permissions.pauseSandbox) {
    const action = sandboxLifecycleAction(sandbox, stale);
    actions.push({
      id: action.kind,
      label: action.label,
      icon: action.kind === 'revive_sandbox' ? <IconPlayOutline16 /> : <IconPauseOutline16 />,
      disabled: busy || !action.allowed,
      reason: busy ? 'A Sandbox action is in progress.' : action.reason,
    });
  }
  if (permissions.deleteSandbox) {
    const reason = sandboxDeletionReason(sandbox, { busy: submitting, stale, lifecycleBusy });
    actions.push({
      id: 'delete_sandbox',
      label: 'Delete',
      icon: <IconTrashOutline16 />,
      danger: true,
      disabled: reason !== undefined,
      reason,
    });
  }
  return actions;
}

export function workspaceActions(
  workspace: WorkspaceDetail,
  permissions: TreePermissions,
  busy: boolean,
  runtimeAvailable = true,
): TreeRowAction[] {
  const actions: TreeRowAction[] = [];
  if (permissions.createAgent)
    actions.push({
      id: 'new_agent',
      label: 'New agent',
      icon: <IconPlusOutline16 />,
      disabled: busy || !runtimeAvailable || workspace.status !== 'ready',
      reason: !runtimeAvailable
        ? 'Available when the Sandbox is active.'
        : workspace.status !== 'ready'
          ? 'The Workspace is not ready.'
          : undefined,
    });
  if (permissions.removeWorkspace)
    actions.push({
      id: 'remove_workspace',
      label: 'Remove',
      icon: <IconTrashOutline16 />,
      danger: true,
      disabled: busy || !runtimeAvailable || workspace.status === 'removing',
      reason: !runtimeAvailable
        ? 'Available when the Sandbox is active.'
        : workspace.status === 'removing'
          ? 'Deletion is already in progress.'
          : undefined,
    });
  return actions;
}

export interface AgentActionContext {
  readonly sandbox?: SandboxSummary;
  readonly viewerId?: string;
  readonly stale?: boolean;
  readonly busy?: boolean;
}

export const OPEN_SANDBOX_TO_RESUME = 'open_sandbox_to_resume';
const ACTION_BUSY = 'Another action is still being confirmed.';
const REQUIRES_ACTIVE_SANDBOX = 'Available when the Sandbox is active.';
// A parked failed runtime update never returns the Sandbox to active by itself.
const PARKED_RUNTIME_UPDATE =
  'The runtime update failed. Delete the environment, or open it to retry the update.';

export function agentActions(
  status: AgentStatus,
  permissions: TreePermissions,
  context: AgentActionContext,
): TreeRowAction[] {
  const { sandbox, viewerId, stale, busy } = context;
  const runtimeAvailable =
    !busy && !stale && sandbox !== undefined && sandboxChildrenAvailable(sandbox);
  const parked = sandbox !== undefined && sandboxHasParkedRuntimeUpdate(sandbox);
  const unavailable = parked ? PARKED_RUNTIME_UPDATE : REQUIRES_ACTIVE_SANDBOX;
  const actions: TreeRowAction[] = [];
  if (permissions.stopAgent)
    actions.push({
      id: 'stop_agent',
      label: 'Stop',
      icon: <IconStopFill16 />,
      disabled:
        !runtimeAvailable ||
        !['starting', 'ready', 'working', 'waiting_approval', 'paused'].includes(status),
      reason: busy ? ACTION_BUSY : !runtimeAvailable ? unavailable : undefined,
    });
  if (permissions.deleteAgent) {
    const terminal = status === 'stopped' || status === 'failed';
    const paused = terminal && sandbox?.status === 'paused' && !sandbox.lifecycle?.operation;
    const reason =
      agentDeletionBlock(context) ??
      (paused
        ? 'Resume the Sandbox before deleting this Agent and its Worktree.'
        : !runtimeAvailable
          ? unavailable
          : !terminal
            ? 'Stop this Agent before deleting it and its Worktree.'
            : undefined);
    if (
      paused &&
      sandbox.lifecycle &&
      !stale &&
      !busy &&
      viewerId !== undefined &&
      sandbox.owner === viewerId
    )
      actions.push({
        id: OPEN_SANDBOX_TO_RESUME,
        label: 'Open Sandbox to resume',
        icon: <IconDataOutline16 />,
        route: { kind: 'sandbox', sandboxId: sandbox.id },
      });
    actions.push({
      id: 'delete_agent',
      label: 'Delete',
      icon: <IconTrashOutline16 />,
      danger: true,
      disabled: reason !== undefined,
      reason,
    });
  }
  // The Agent cannot be deleted without its runtime; its environment can.
  if (permissions.deleteSandbox && sandbox !== undefined && parked) {
    const reason = sandboxDeletionReason(sandbox, { busy, stale });
    actions.push({
      id: 'delete_sandbox',
      label: 'Delete environment',
      icon: <IconTrashOutline16 />,
      danger: true,
      disabled: reason !== undefined,
      reason,
    });
  }
  if (permissions.exportHistory) {
    actions.push({
      id: 'export_retained_conversation',
      label: 'Export retained conversation',
      icon: <IconDownloadOutline16 />,
    });
    actions.push({
      id: 'export_history',
      label: 'Export raw history',
      icon: <IconDownloadOutline16 />,
      disabled: !runtimeAvailable,
      reason: !runtimeAvailable
        ? 'Raw history requires an available runtime. Use Export retained conversation.'
        : undefined,
    });
  }
  return actions;
}

function agentDeletionBlock({ sandbox, stale, busy }: AgentActionContext): string | undefined {
  if (stale || sandbox === undefined)
    return 'Sandbox status is unavailable. Refresh and try again.';
  if (sandbox.status === 'retiring') return 'The Sandbox is being deleted.';
  if (sandbox.status === 'retired') return 'The Sandbox has been deleted.';
  if (sandbox.status === 'reviving' || sandbox.lifecycle?.operation?.purpose === 'revive')
    return 'Wait for the Sandbox to finish resuming.';
  if (busy) return ACTION_BUSY;
}
