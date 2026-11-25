import { ApiError } from '@cma/client';
import type { AgentV2Session } from './client';
import type { QueuedInputIntent } from './queuedInputJournal';

export async function sendQueuedInputIntent(
  session: AgentV2Session,
  intent: QueuedInputIntent,
  signal: AbortSignal,
) {
  await session.authorize(signal);
  const body = {
    source_command_id: intent.commandId,
    expected_command_revision: intent.revision,
  };
  let result;
  try {
    if (intent.action === 'edit')
      result = await session.responses.editQueuedInput(
        session.journal.agentId,
        intent.responseId,
        { ...body, text: intent.text! },
        { idempotencyKey: intent.key, signal },
      );
    else
      result = await session.responses.cancelQueuedResponse(
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
      if (intent.action === 'edit' && error.code === 'queued_input_not_admitted') {
        session.journal.queuedInputs.settle(intent, true);
        return;
      }
      session.journal.queuedInputs.settle(intent);
    }
    throw error;
  }
  await session.authorize(signal);
  session.journal.queuedInputs.settle(intent);
  return {
    response: result.response,
    row:
      'command_revision' in result
        ? {
            commandId: intent.commandId,
            responseId: intent.responseId,
            revision: result.command_revision,
            input: result.text,
            busy: true,
          }
        : undefined,
  };
}
