import type { DeletionObservation, DeletionRequest } from '../deletion';

export class DeletionBlocked extends Error {}

export const stoppedProofMissing =
  'Stopping has not been confirmed. This request is blocked. Review the parent environment before continuing.';

export function deleteMayHaveBeenSent(request: DeletionRequest): boolean {
  // Older combined receipts cannot prove that Delete was never sent.
  return request.stop !== undefined && request.deleteSent !== false;
}

export function blockedBeforeDelete(request: DeletionRequest): boolean {
  return request.stop?.phase === 'blocked' && !deleteMayHaveBeenSent(request);
}

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
  if (blockedBeforeDelete(request)) return stoppedProofMissing;
  if (observation.blocked) return observation.blocked;
  if (observation.agent?.status === 'failed' || observation.agent?.status === 'deleted')
    return deleteMayHaveBeenSent(request)
      ? 'The delete result is unconfirmed. Review the parent environment before retrying the original request.'
      : stoppedProofMissing;
}
