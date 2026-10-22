import type { AgentResourceState } from './controllerTypes';
import { transientReadFailure } from './loadAgent';
import { ApiError } from '@chronoai/cma-sdk';
import { IdentityChangedError, isIdentityChanged } from './transport/identity';

export function errorMessage(error: unknown): string {
  if (isIdentityChanged(error)) return new IdentityChangedError().message;
  if (error instanceof ApiError && error.code === 'sandbox_checkpoint_in_progress')
    return 'Progress is being saved. Your input was not accepted. Try again after saving finishes.';
  return error instanceof Error ? error.message : 'The Agent operation did not complete.';
}

/** A transient projection failure must not cancel an already admitted send. */
export function retainHistoryOnFailure(
  error: unknown,
  current: AgentResourceState,
  agentId: string,
): AgentResourceState | undefined {
  return transientReadFailure(error) && current.status === 'ready' && current.detail.id === agentId
    ? { ...current, historyLoading: false, historyUnavailable: true }
    : undefined;
}
