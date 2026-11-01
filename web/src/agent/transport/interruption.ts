import { ApiError } from '@chronoai/cma-sdk';
import type { ConversationState } from '../model';
import type { AgentV2Session } from './client';
import type { InterruptionRecord } from './interruptionJournal';

/** Local observation/retention failed before any interruption POST was attempted. */
export class InterruptionNotSubmittedError extends Error {
  constructor(cause: unknown) {
    super(cause instanceof Error ? cause.message : 'The interruption could not be submitted.', {
      cause,
    });
    this.name = 'InterruptionNotSubmittedError';
  }
}

export async function submitV2Interruption(
  session: AgentV2Session,
  target: ConversationState['responseBinding'],
  text: string,
  key: string,
  signal: AbortSignal,
) {
  return submitV2InterruptionAttempt(session, target, text, key, signal, true);
}

/** Re-observe once when turn progress made the original revision stale. */
async function submitV2InterruptionAttempt(
  session: AgentV2Session,
  target: ConversationState['responseBinding'],
  text: string,
  key: string,
  signal: AbortSignal,
  allowRevisionRetry: boolean,
) {
  await session.authorize(signal);
  let intent;
  try {
    intent = await prepare(session, target, text, key, signal);
  } catch (cause) {
    throw new InterruptionNotSubmittedError(cause);
  }
  if (intent === undefined) return undefined;
  try {
    return await send(session, intent, signal, false);
  } catch (error) {
    if (
      allowRevisionRetry &&
      error instanceof ApiError &&
      error.status === 409 &&
      error.code === 'response_interrupt_not_admitted'
    ) {
      session.journal.interruption.refused(intent.key);
      return submitV2InterruptionAttempt(session, target, text, retryKey(), signal, false);
    }
    if (isDefinitiveRefusal(error)) session.journal.interruption.refused(intent.key);
    throw error;
  }
}

async function prepare(
  session: AgentV2Session,
  target: ConversationState['responseBinding'],
  text: string,
  key: string,
  signal: AbortSignal,
) {
  if (
    session.journal.read()?.state === 'pending' ||
    session.journal.readControl()?.commandId === null
  )
    throw new Error('Recover your pending request before interrupting this turn.');
  if (!target) throw new Error('This turn does not support response interruption.');
  const previous = session.journal.interruption.read();
  let intent: InterruptionRecord;
  if (previous?.receipt === null) {
    if (
      previous.responseId !== target.responseId ||
      previous.body.turn_sequence !== target.turnSequence ||
      previous.body.text !== text
    )
      throw new Error('Recover your pending interruption before sending another input.');
    intent = previous;
  } else {
    const observed = await session.responses.observeControls(
      session.journal.agentId,
      target.responseId,
      { signal },
    );
    await session.authorize(signal);
    if (
      observed.target?.response_id !== target.responseId ||
      observed.target.turn_sequence !== target.turnSequence
    )
      throw new Error(
        'The response target changed. Review the conversation before sending another input.',
      );
    // This read conclusively precedes any interruption intent or mutation. The
    // caller can submit ordinary input without ever retargeting cancellation.
    if (!observed.target_active) return undefined;
    if (!observed.runtime_support.cancel)
      throw new Error('This runtime does not support interrupting the current turn.');
    intent = session.journal.interruption.retain({
      key,
      responseId: target.responseId,
      receipt: null,
      body: {
        turn_sequence: target.turnSequence,
        expected_agent_revision: observed.agent_revision,
        text,
      },
    });
  }
  return intent;
}

export async function recoverV2Interruption(session: AgentV2Session, signal: AbortSignal) {
  await session.authorize(signal);
  const intent = session.journal.interruption.read();
  if (intent?.receipt === null) await send(session, intent, signal);
}

function retryKey(): string {
  return typeof crypto.randomUUID === 'function'
    ? crypto.randomUUID()
    : `interrupt-retry-${String(Date.now())}-${String(Math.random()).slice(2)}`;
}

function isDefinitiveRefusal(error: unknown): error is ApiError {
  return (
    error instanceof ApiError &&
    ((error.status === 409 && error.code === 'response_interrupt_not_admitted') ||
      (error.status === 422 &&
        ['invalid_interrupt_request', 'capability_unsupported'].includes(error.code ?? '')))
  );
}

async function send(
  session: AgentV2Session,
  intent: InterruptionRecord,
  signal: AbortSignal,
  clearDefinitiveRefusal = true,
) {
  await session.authorize(signal);
  let result;
  try {
    result = await session.responses.interruptWithInput(
      session.journal.agentId,
      intent.responseId,
      intent.body,
      { idempotencyKey: intent.key, signal },
    );
  } catch (error) {
    if (clearDefinitiveRefusal && isDefinitiveRefusal(error)) {
      await session.authorize(signal);
      session.journal.interruption.refused(intent.key);
    }
    throw error;
  }
  await session.authorize(signal);
  session.journal.interruption.accepted(intent.key, {
    controlId: result.control.receipt.command_id,
    commandId: result.response.receipt.command_id,
    responseId: result.response.receipt.response_id!,
  });
  return result;
}
