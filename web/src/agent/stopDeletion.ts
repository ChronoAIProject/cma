import type { DeletionObservation, DeletionRequest } from '../deletion';

export class DeletionBlocked extends Error {}

export const stoppedProofMissing =
  'Stopping has not been confirmed. This request is blocked. Review the parent environment before continuing.';

export function stopDeletionBlock(
  request: DeletionRequest,
  observation: DeletionObservation,
): string | undefined {
  const stop = request.stop;
  if (!stop) return undefined;
  if (
    observation.sandboxId !== stop.sandboxId ||
    observation.agent?.workspaceId !== stop.workspaceId ||
    observation.agent?.worktreeId !== stop.worktreeId
  )
    return 'The Agent deletion target changed. Review the parent environment.';
  if (request.accepted || observation.complete) return undefined;
  if (stop.phase === 'blocked') return stoppedProofMissing;
  if (observation.blocked) return observation.blocked;
  if (observation.agent?.status === 'failed' || observation.agent?.status === 'deleted')
    return stoppedProofMissing;
}
