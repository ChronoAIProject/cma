import { ApiError } from '@cma/client';
import type { AgentV2Session } from './client';
import type { QueuedCancellationRecord } from './queuedJournal';

export interface QueuedMessageTarget {
  readonly responseId: string;
  readonly commandId: string;
}

export async function cancelQueuedMessage(
  session: AgentV2Session,
  target: QueuedMessageTarget,
  key: string,
  signal: AbortSignal,
): Promise<void> {
  await session.authorize(signal);
  if (session.journal.queuedCancellation.read()?.commandId === null)
    throw new Error('Recover your pending cancellation before requesting another cancellation.');
  const observed = await session.responses.observeQueuedCancellation(
    session.journal.agentId,
    target.responseId,
    { signal },
  );
  await session.authorize(signal);
  if (observed.response.receipt.command_id !== target.commandId || !observed.cancellable)
    throw new Error(
      'This message can no longer be cancelled before it starts. Review its recorded status.',
    );
  const intent = session.journal.queuedCancellation.retain({
    key,
    responseId: target.responseId,
    commandId: null,
    body: {
      source_command_id: target.commandId,
      expected_command_revision: observed.command_revision,
    },
  });
  await send(session, intent, signal);
}

export async function recoverQueuedCancellation(session: AgentV2Session, signal: AbortSignal) {
  await session.authorize(signal);
  const intent = session.journal.queuedCancellation.read();
  if (intent?.commandId === null) return send(session, intent, signal);
}

async function send(
  session: AgentV2Session,
  intent: QueuedCancellationRecord,
  signal: AbortSignal,
) {
  await session.authorize(signal);
  let result;
  try {
    result = await session.responses.cancelQueuedResponse(
      session.journal.agentId,
      intent.responseId,
      intent.body,
      { idempotencyKey: intent.key, signal },
    );
  } catch (error) {
    // A conflict can hide a successful commit even on the first browser call.
    if (
      error instanceof ApiError &&
      ((error.status === 409 && error.code === 'queued_response_cancellation_not_admitted') ||
        (error.status === 422 && error.code === 'invalid_queued_cancellation_request'))
    )
      session.journal.queuedCancellation.refused(intent.key);
    throw error;
  }
  await session.authorize(signal);
  session.journal.queuedCancellation.accepted(intent.key, result.cancellation.receipt.command_id);
  return result.response;
}
