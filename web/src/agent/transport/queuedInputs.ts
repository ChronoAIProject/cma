import { ApiError } from '@chronoai/cma-sdk';
import type { AgentV2Session } from './client';
import type { QueuedInputIntent } from './queuedInputJournal';

export async function sendQueuedInputIntent(
  session: AgentV2Session,
  intent: QueuedInputIntent,
  signal: AbortSignal,
): Promise<void> {
  await session.authorize(signal);
  const body = {
    source_command_id: intent.commandId,
    expected_command_revision: intent.revision,
  };
  try {
    if (intent.action === 'edit')
      await session.responses.editQueuedInput(
        session.journal.agentId,
        intent.responseId,
        { ...body, text: intent.text! },
        { idempotencyKey: intent.key, signal },
      );
    else
      await session.responses.cancelQueuedResponse(
        session.journal.agentId,
        intent.responseId,
        body,
        { idempotencyKey: intent.key, signal },
      );
  } catch (error) {
    // A generic conflict or lost reply may conceal a successful transaction.
    // Only explicit no-admission codes permit dropping the exact recovery intent.
    if (
      error instanceof ApiError &&
      ((error.status === 409 &&
        ['queued_input_not_admitted', 'queued_response_cancellation_not_admitted'].includes(
          error.code ?? '',
        )) ||
        (error.status === 422 &&
          ['invalid_queued_input_request', 'invalid_queued_cancellation_request'].includes(
            error.code ?? '',
          )))
    ) {
      await session.authorize(signal);
      session.journal.queuedInputs.settle(intent);
    }
    throw error;
  }
  await session.authorize(signal);
  session.journal.queuedInputs.settle(intent);
}
