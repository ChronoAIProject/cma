import type { AgentStatus, SandboxSummary, WorkspaceDetail } from '../api/types';
import { sandboxChildrenAvailable, sandboxLifecycleAction } from '../sandbox/lifecycle';
import {
  IconPauseOutline16,
  IconPlayOutline16,
  IconPlusOutline16,
  IconDownloadOutline16,
  IconStopFill16,
  IconTrashOutline16,
} from '../ui/icons';
import type { TreePermissions } from './Tree';
import type { TreeRowAction } from './TreeRow';

export function sandboxActions(
  sandbox: SandboxSummary,
  permissions: TreePermissions,
  stale: boolean,
  busy: boolean,
): TreeRowAction[] {
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
  if (permissions.deleteSandbox)
    actions.push({
      id: 'delete_sandbox',
      label: 'Delete',
      icon: <IconTrashOutline16 />,
      danger: true,
      disabled:
        busy ||
        stale ||
        sandbox.status === 'retiring' ||
        sandbox.lifecycle?.checkpoint !== undefined ||
        sandbox.lifecycle?.operation !== undefined,
      reason:
        sandbox.status === 'retiring'
          ? 'Deletion is already in progress.'
          : sandbox.lifecycle?.checkpoint !== undefined ||
              sandbox.lifecycle?.operation !== undefined
            ? 'Wait for the current Sandbox operation to finish.'
            : undefined,
    });
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

export function agentActions(
  status: AgentStatus,
  permissions: TreePermissions,
  runtimeAvailable = true,
): TreeRowAction[] {
  const actions: TreeRowAction[] = [];
  if (permissions.stopAgent)
    actions.push({
      id: 'stop_agent',
      label: 'Stop',
      icon: <IconStopFill16 />,
      disabled:
        !runtimeAvailable ||
        !['starting', 'ready', 'working', 'waiting_approval', 'paused'].includes(status),
      reason: !runtimeAvailable ? 'Available when the Sandbox is active.' : undefined,
    });
  if (permissions.deleteAgent)
    actions.push({
      id: 'delete_agent',
      label: 'Delete',
      icon: <IconTrashOutline16 />,
      danger: true,
      disabled: !runtimeAvailable || !['stopped', 'failed'].includes(status),
      reason: !runtimeAvailable
        ? 'Available when the Sandbox is active.'
        : !['stopped', 'failed'].includes(status)
          ? 'Stop this Agent before deleting it and its Worktree.'
          : undefined,
    });
  if (permissions.exportHistory)
    actions.push({
      id: 'export_history',
      label: 'Export history',
      icon: <IconDownloadOutline16 />,
    });
  return actions;
}
