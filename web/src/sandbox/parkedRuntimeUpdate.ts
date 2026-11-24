import type { SandboxLifecycle, SandboxRuntimeUpdate, SandboxStatus } from '../api/types';

type Operation = NonNullable<SandboxLifecycle['operation']>;

export interface SandboxRuntimeObservation {
  readonly status: SandboxStatus;
  readonly lifecycle?: {
    readonly operation?: Pick<Operation, 'purpose'> & Partial<Pick<Operation, 'startedAt'>>;
    readonly result?: SandboxLifecycle['result'];
  };
  readonly runtimeUpdate?: SandboxRuntimeUpdate;
}

/** Presentation evidence from one authorized observation; never an action permission. */
export function sandboxHasParkedRuntimeUpdate(sandbox: SandboxRuntimeObservation): boolean {
  const { status, lifecycle, runtimeUpdate } = sandbox;
  const operation = lifecycle?.operation;
  if (!(
    (status === 'rotating' && operation?.purpose === 'image_rotation') ||
    (status === 'reviving' && operation?.purpose === 'revive')
  ))
    return false;
  const result = lifecycle?.result;
  if (
    runtimeUpdate !== undefined &&
    (runtimeUpdate.state !== 'failed' || runtimeUpdate.completedAt !== undefined)
  )
    return false;
  if (result?.startedAt !== undefined && result.startedAt !== operation?.startedAt) return false;
  if (result?.code === 'runtime_update_parked')
    return result.state === 'failed' && !result.continuing && result.startedAt !== undefined;
  if (result?.state === 'failed') return false;
  // Older servers report an ongoing lifecycle result even after a rotation parks.
  // Their detail projection has no operation ID/target to compare.
  return (
    status === 'rotating' &&
    runtimeUpdate?.state === 'failed' &&
    runtimeUpdate.completedAt === undefined &&
    (runtimeUpdate.startedAt === undefined || runtimeUpdate.startedAt === operation?.startedAt)
  );
}
