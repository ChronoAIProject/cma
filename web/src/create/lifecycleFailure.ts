import { ApiError } from '@chronoai/cma-sdk';
import { AgentLifecycleCreateError } from '../api/client';

const identityMessages = new Set([
  'Invalid operation identity',
  'Agent setup identity could not be verified.',
  'Agent setup identity changed. Your original request has been saved.',
  'Agent Manager returned an invalid lifecycle operation.',
]);

export type LifecycleFailure =
  | { readonly kind: 'message'; readonly message: string }
  | { readonly kind: 'unconfirmed' | 'status-unavailable'; readonly task: boolean };

export interface LifecycleFailureAction {
  readonly label: string;
  readonly kind: 'recover' | 'status';
}

/** Classify presentation only; the lifecycle hook retains its existing retry decisions. */
export function lifecycleFailure(
  error: unknown,
  phase: 'submit' | 'status',
  task: boolean,
): LifecycleFailure {
  if (error instanceof Error && identityMessages.has(error.message))
    return { kind: 'message', message: error.message };
  if (error instanceof ApiError || error instanceof AgentLifecycleCreateError) {
    const message = error.message.trim();
    const noDetail =
      !message ||
      message === error.code ||
      /^[a-z][a-z0-9]*_[a-z0-9_]+$/.test(message) ||
      /^(?:HTTP \d{3}(?: problem document carried no detail)?$|Agent lifecycle status failed \(\d{3}\)\.|Agent Manager could not create this Agent \(\d{3}\)\.|Internal Server Error$|Bad Gateway$|Service Unavailable$|Gateway Timeout$|Request Timeout$)/i.test(
        message,
      );
    if (![408, 504].includes(error.status) && !noDetail)
      return { kind: 'message', message: error.message };
    if (
      phase === 'submit' &&
      error instanceof AgentLifecycleCreateError &&
      [400, 422].includes(error.status)
    )
      return {
        kind: 'message',
        message: task
          ? 'Your task could not be started. Review it and try again.'
          : 'Your Agent could not be created. Review its settings and try again.',
      };
    if (phase === 'submit' && error.status < 500 && ![408, 429].includes(error.status))
      return {
        kind: 'message',
        message: `Agent Manager refused this ${task ? 'task' : 'Agent creation'} (${String(error.status)}). Your original request is saved.`,
      };
  }
  return { kind: phase === 'status' ? 'status-unavailable' : 'unconfirmed', task };
}

/** The view supplies its actual button and whether it reads status or resends a request. */
export function lifecycleFailureMessage(
  failure: LifecycleFailure | undefined,
  action: LifecycleFailureAction,
): string | undefined {
  if (!failure) return undefined;
  if (failure.kind === 'message') return failure.message;
  if (failure.kind === 'status-unavailable')
    return `${failure.task ? 'Task' : 'Agent'} status couldn't be read. Use ${action.label} to check again.`;
  if (action.kind === 'status')
    return `We couldn't confirm your ${failure.task ? 'task' : 'Agent'} setup. Use ${action.label} to check again.`;
  return `We couldn't confirm your ${failure.task ? 'task was received' : 'Agent was created'}. Use ${action.label} to resend the same request.`;
}
