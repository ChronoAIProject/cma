import type { AgentStatus, SandboxLifecycle, SandboxSummary, WorkspaceStatus } from '../api/types';
import { sandboxStatusPresentation, type SandboxStatusObservation } from './presentation';
import { sandboxHasParkedRuntimeUpdate } from './parkedRuntimeUpdate';

export function transitionRefreshDelay(startedAt: number): number {
  return Date.now() - startedAt < 10_000 ? 250 : 1500;
}

export function sandboxChildrenAvailable(sandbox: SandboxSummary): boolean {
  return (
    sandbox.status === 'active' &&
    sandbox.lifecycle !== undefined &&
    sandbox.lifecycle.operation === undefined
  );
}

export function sandboxRetirementDuringRevive(sandbox: SandboxSummary): boolean {
  return (
    sandbox.status === 'reviving' && sandbox.lifecycle?.result?.actions.includes('retire') === true
  );
}

export interface SandboxRetirementObservation {
  readonly status?: SandboxSummary['status'];
  readonly lifecycle?: {
    readonly result?: Pick<
      NonNullable<SandboxLifecycle['result']>,
      'state' | 'code' | 'continuing'
    >;
  };
}

export function sandboxRetirementWaitExceeded(sandbox: SandboxRetirementObservation): boolean {
  const result = sandbox.lifecycle?.result;
  return (
    sandbox.status === 'retiring' &&
    result?.state === 'recoverable_pending' &&
    result.code === 'lifecycle_wait_exceeded' &&
    result.continuing
  );
}

export function sandboxChildrenReadable(sandbox: SandboxSummary): boolean {
  return [
    'active',
    'credential_rotating',
    'paused',
    'pausing',
    'reviving',
    'rotating',
    'failed',
  ].includes(sandbox.status);
}

export function sandboxChildStatus(
  sandbox: SandboxStatusObservation,
  status: AgentStatus | WorkspaceStatus,
) {
  if (['stopped', 'failed', 'deleted', 'removed', 'removing'].includes(status)) return status;
  const parent = sandboxStatusPresentation(sandbox).status;
  return parent === 'active' || parent === 'credential_rotating' ? status : parent;
}

export function sandboxUnavailableNotice(sandbox: SandboxSummary): string | undefined {
  if (sandboxChildrenAvailable(sandbox)) return undefined;
  if (sandbox.status === 'failed')
    return 'This Sandbox is unavailable. Review the recorded failure below.';
  if (
    sandboxChildrenReadable(sandbox) &&
    (sandbox.lifecycle?.operation !== undefined ||
      ['pausing', 'reviving', 'rotating'].includes(sandbox.status))
  )
    return `${sandboxLifecycleLabel(sandbox)}. Agent conversations and history remain available.`;
  if (sandbox.status === 'paused')
    return 'This Sandbox is paused. Your work is preserved. Open an Agent to read its conversation, or resume the Sandbox to continue working.';
  if (sandbox.status === 'credential_rotating')
    return 'Service access is updating. Your Agents and conversations remain available.';
  if (sandbox.status === 'retired')
    return 'This Sandbox has been deleted. Its runtime is gone; retained history remains read-only.';
  if (sandbox.status === 'retiring')
    return 'This Sandbox is being deleted. Its Workspaces and Agents are unavailable during cleanup.';
  if (sandbox.status === 'active')
    return 'Sandbox action availability is not reported. Agent conversations and history remain available.';
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
  return sandboxStatusPresentation(sandbox).label;
}

export function sandboxTone(sandbox: SandboxSummary) {
  return sandboxStatusPresentation(sandbox).tone;
}

export function sandboxRefreshDelay(sandbox: SandboxSummary, childrenChanging = false): number {
  if (sandboxHasParkedRuntimeUpdate(sandbox)) return 5000;
  return childrenChanging ||
    sandbox.lifecycle?.checkpoint !== undefined ||
    ['queued', 'updating'].includes(sandbox.runtimeUpdate?.state ?? '') ||
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
