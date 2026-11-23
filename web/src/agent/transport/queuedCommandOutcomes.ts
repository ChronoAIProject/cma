import type { V2CommandStatus } from '@cma/client';

export const queuedOutcomeText = {
  unknown: 'Message status could not be confirmed. Refresh status.',
  waiting: 'Message is waiting to start. Its queue details are being refreshed.',
  starting: 'Message is being prepared. Execution is not yet confirmed.',
  running: 'Message has started. Conversation history is being refreshed.',
  completed: 'Message completed. Conversation history is being refreshed.',
  expired: 'Expired before starting',
  cancelled: 'Message was cancelled.',
  failed: 'Message failed. Review the conversation before submitting it again.',
  uncertain:
    'Message execution could not be confirmed. Review the conversation before trying again.',
  access:
    'Message could not continue because access was unavailable. Review access before trying again.',
  setup:
    'Message could not continue because setup is required. Review connections and credentials.',
} as const;

export interface QueuedInputOutcome {
  readonly commandId: string;
  readonly responseId: string;
  readonly input?: string;
  readonly kind: keyof typeof queuedOutcomeText;
  readonly terminal: boolean;
}

export function commandOutcome(
  status: V2CommandStatus,
): Pick<QueuedInputOutcome, 'kind' | 'terminal'> {
  const terminal = ['completed', 'failed', 'cancelled'].includes(status.status);
  if (status.status === 'failed') {
    if (status.failure === 'queue_timeout' && status.delivery === null)
      return { kind: 'expired', terminal };
    if (
      status.failure === 'runtime_lost' ||
      status.delivery === 'runtime_lost' ||
      status.delivery === 'recovery_required'
    )
      return { kind: 'uncertain', terminal };
    if (status.failure === 'authority_revoked' || status.failure === 'authority_unavailable')
      return { kind: 'access', terminal };
    if (status.failure === 'credential_setup_required' || status.failure === 'connections_required')
      return { kind: 'setup', terminal };
    return { kind: 'failed', terminal };
  }
  if (status.status === 'cancelled' || status.status === 'completed')
    return { kind: status.status, terminal };
  if (status.delivery === 'runtime_lost' || status.delivery === 'recovery_required')
    return { kind: 'uncertain', terminal };
  if (status.delivery === 'dispatch_fenced' || status.delivery === 'write_confirmed')
    return { kind: 'starting', terminal };
  return { kind: status.delivery === 'execution_observed' ? 'running' : 'waiting', terminal };
}
