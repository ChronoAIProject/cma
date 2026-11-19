import type { SandboxSummary } from '../api/types';
import { sandboxRetirementDuringRevive } from './lifecycle';

export function sandboxHasParkedRuntimeUpdate(sandbox: SandboxSummary): boolean {
  return (
    sandbox.status === 'rotating' &&
    sandbox.lifecycle?.operation?.purpose === 'image_rotation' &&
    sandbox.runtimeUpdate?.state === 'failed'
  );
}

export function sandboxDeletionReason(
  sandbox: SandboxSummary,
  context: {
    readonly busy?: boolean;
    readonly stale?: boolean;
    readonly lifecycleBusy?: boolean;
  } = {},
): string | undefined {
  if (sandbox.status === 'retired') return 'This Sandbox has been deleted.';
  if (sandbox.status === 'retiring') return 'Deletion is already in progress.';
  if (context.stale) return 'Checking the current Sandbox status.';
  const canCancelOperation =
    sandboxRetirementDuringRevive(sandbox) || sandboxHasParkedRuntimeUpdate(sandbox);
  if (
    context.busy ||
    sandbox.lifecycle?.checkpoint !== undefined ||
    ((context.lifecycleBusy || sandbox.lifecycle?.operation !== undefined) && !canCancelOperation)
  )
    return 'Wait for the current Sandbox operation to finish.';
  return undefined;
}
