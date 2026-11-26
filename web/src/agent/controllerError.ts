import type { AgentResourceState } from './controllerTypes';
import { transientReadFailure } from './loadAgent';
import { ApiError } from '@cma/client';
import { IdentityChangedError, isIdentityChanged } from './transport/identity';
import { approvalBaselineMessage } from './transport/approvalBaseline';
import { readableError } from '../vocabulary/errors';
import { WorkspaceReadUnavailable } from '../api/rest/workspaceRead';
import { currentAgentStatus } from './status';
import {
  RESTART_AVAILABLE_MESSAGE,
  STOPPED_AGENT_MESSAGE,
  STOP_RESPONSE_GUIDANCE,
} from './stopAgentCopy';

export const WORKSPACE_READ_RECONNECTING =
  'Workspace status is temporarily unavailable (HTTP 503). Reconnecting.';

export function agentHistoryIssue(
  state: Extract<AgentResourceState, { status: 'ready' }>,
): string | undefined {
  if (state.historyLoading) return 'Loading conversation activity.';
  if (state.historyUnavailable) return 'Earlier activity could not be loaded. Retrying…';
}

export function errorMessage(error: unknown): string {
  const approval = approvalBaselineMessage(error);
  if (approval !== undefined) return approval;
  if (isIdentityChanged(error)) return new IdentityChangedError().message;
  if (error instanceof WorkspaceReadUnavailable) return `${error.message} Check again shortly.`;
  if (error instanceof ApiError && error.code === 'sandbox_checkpoint_in_progress')
    return 'Progress is being saved. Your input was not accepted. Try again after saving finishes.';
  return readableError(error, 'The Agent operation did not complete.');
}

export function isLegacySuggestionRefusal(error: unknown): error is ApiError {
  return (
    error instanceof ApiError &&
    ((error.status === 422 &&
      ['invalid_run_request', 'invalid_answer_context'].includes(error.code ?? '')) ||
      (error.status === 409 && error.code === 'agent_not_ready'))
  );
}

export function isV2InputRefusal(error: unknown, interrupting: boolean): boolean {
  return (
    error instanceof ApiError &&
    ((error.status === 422 &&
      ([
        'invalid_response_input',
        'invalid_answer_context',
        'invalid_control_request',
        'invalid_interrupt_request',
      ].includes(error.code ?? '') ||
        (interrupting && error.code === 'capability_unsupported'))) ||
      (error.status === 409 &&
        [
          'response_control_not_admitted',
          'response_interrupt_not_admitted',
          'agent_command_not_admissible',
        ].includes(error.code ?? '')))
  );
}

export function stoppedInputMessage(
  error: unknown,
  current: AgentResourceState,
  agentId: string,
): string | undefined {
  if (
    error instanceof ApiError &&
    error.status === 409 &&
    (error.code === 'agent_not_ready' || error.code === 'agent_command_not_admissible') &&
    current.status === 'ready' &&
    current.detail.id === agentId &&
    currentAgentStatus(current.detail, current.conversation) === 'stopped'
  )
    return current.detail.restartAvailable
      ? RESTART_AVAILABLE_MESSAGE
      : `${STOPPED_AGENT_MESSAGE} ${STOP_RESPONSE_GUIDANCE}`;
}

/** A transient projection failure must not cancel an already admitted send. */
export function retainHistoryOnFailure(
  error: unknown,
  current: AgentResourceState,
  agentId: string,
): AgentResourceState | undefined {
  return transientReadFailure(error) && current.status === 'ready' && current.detail.id === agentId
    ? {
        ...current,
        historyLoading: false,
        historyUnavailable: true,
        ...(error instanceof WorkspaceReadUnavailable ? { workspaceUnavailable: true } : {}),
      }
    : undefined;
}

export function agentReadRetryAt(state: AgentResourceState): number {
  return state.status === 'error'
    ? (state.retryAt ?? 0)
    : state.status === 'ready'
      ? (state.historyRetryAt ?? 0)
      : 0;
}
