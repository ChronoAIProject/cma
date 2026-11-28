import type { SandboxSummary } from '../api/types';
import { sandboxRetirementDuringRevive } from './lifecycle';
import { sandboxHasParkedRuntimeUpdate } from './parkedRuntimeUpdate';
export { sandboxHasParkedRuntimeUpdate } from './parkedRuntimeUpdate';

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
  const operation = sandbox.lifecycle?.operation;
  const result = sandbox.lifecycle?.result;
  const canRetireStalledRotation =
    sandbox.status === 'rotating' &&
    operation?.purpose === 'image_rotation' &&
    result?.code === 'runtime_update_stalled' &&
    result.state === 'recoverable_pending' &&
    result.continuing &&
    result.startedAt === operation.startedAt &&
    result.actions.includes('retire');
  const canCancelOperation =
    canRetireStalledRotation ||
    sandboxRetirementDuringRevive(sandbox) ||
    (sandbox.status === 'rotating' && sandboxHasParkedRuntimeUpdate(sandbox));
  if (
    context.busy ||
    sandbox.lifecycle?.checkpoint !== undefined ||
    ((context.lifecycleBusy || sandbox.lifecycle?.operation !== undefined) && !canCancelOperation)
  )
    return 'Wait for the current Sandbox operation to finish.';
  return undefined;
}
