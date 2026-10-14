import type { SandboxSummary } from '../api/types';
import { statusLabel } from '../vocabulary/status';

export function transitionRefreshDelay(startedAt: number): number {
  return Date.now() - startedAt < 10_000 ? 250 : 1500;
}

export function sandboxChildrenAvailable(sandbox: SandboxSummary): boolean {
  return sandbox.status === 'active' && sandbox.lifecycle?.operation === undefined;
}

export function sandboxUnavailableNotice(sandbox: SandboxSummary): string | undefined {
  if (sandboxChildrenAvailable(sandbox)) return undefined;
  if (sandbox.status === 'paused')
    return 'This Sandbox is paused. Resume it to access its Workspaces and Agents. Your work is preserved.';
  if (sandbox.status === 'retired')
    return 'This Sandbox has been deleted. Its runtime is gone; retained history remains read-only.';
  if (sandbox.status === 'retiring')
    return 'This Sandbox is being deleted. Its Workspaces and Agents are unavailable during cleanup.';
  if (sandbox.status === 'failed')
    return 'This Sandbox is unavailable. Review the recorded failure below.';
  return 'Workspaces and Agents will be available when the Sandbox finishes preparation.';
}

export function sandboxLifecycleAction(sandbox: SandboxSummary, stale = false) {
  const resume = sandbox.status === 'paused' || sandbox.status === 'reviving';
  const eligibility = resume ? sandbox.lifecycle?.revive : sandbox.lifecycle?.pause;
  let reason: string | undefined;
  if (stale) reason = 'Status is unavailable. Reconnecting.';
  else if (eligibility === undefined) reason = 'Action availability is not reported.';
  else if (!eligibility.allowed) {
    switch (eligibility.reason) {
      case 'checkpoint_in_progress':
        reason = 'Progress is being saved. Pause will be available when saving finishes.';
        break;
      case 'work_in_progress':
        reason = 'Sandbox work is in progress. This action will be available when it finishes.';
        break;
      case 'not_active':
        reason = 'Pause is available when the sandbox is active.';
        break;
      case 'not_paused':
        reason = 'Resume is available when the sandbox is paused.';
        break;
      default:
        reason = 'This action is currently unavailable.';
    }
  }
  return {
    kind: resume ? ('revive_sandbox' as const) : ('pause_sandbox' as const),
    label: resume ? 'Resume' : 'Pause',
    allowed: !stale && eligibility?.allowed === true,
    reason,
  };
}

export function sandboxLifecycleLabel(sandbox: SandboxSummary): string {
  if (sandbox.status === 'retiring') return 'Deleting';
  if (sandbox.status === 'retired') return 'Deleted';
  if (sandbox.lifecycle?.operation !== undefined) {
    switch (sandbox.lifecycle.operation.purpose) {
      case 'pause':
        return 'Pausing';
      case 'revive':
        return 'Resuming';
      case 'image_rotation':
        return 'Resuming';
    }
  }
  return statusLabel(sandbox.status);
}

export function sandboxRefreshDelay(sandbox: SandboxSummary): number {
  return sandbox.lifecycle?.checkpoint !== undefined ||
    sandbox.lifecycle?.backgroundSave !== undefined ||
    sandbox.lifecycle?.operation !== undefined ||
    [
      'provisioning',
      'installing',
      'pausing',
      'reviving',
      'rotating',
      'credential_rotating',
      'retiring',
    ].includes(sandbox.status) ||
    sandbox.lifecycle?.pause.reason === 'work_in_progress' ||
    sandbox.lifecycle?.revive.reason === 'work_in_progress'
    ? 1500
    : 5000;
}
