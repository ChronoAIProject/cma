import { ApiError, type V2ResponseControlCommand, type V2ResponseControl } from '@chronoai/cma-sdk';
import type { ConversationState } from '../model';
import type { AgentV2Session } from './client';
import type { ControlRecord } from './controlRecord';

export async function submitV2Control(
  session: AgentV2Session,
  target: ConversationState['responseBinding'],
  command: V2ResponseControlCommand,
  key: string,
  signal: AbortSignal,
): Promise<V2ResponseControl> {
  return submitV2ControlAttempt(session, target, command, key, signal, true);
}

/**
 * A response can remain active while the Agent revision advances with tool
 * progress. Rebase one definitive stale-revision refusal onto a fresh
 * observation before surfacing an error to the user. The first request has no
 * durable receipt in this case, so replacing its local journal entry is safe.
 */
async function submitV2ControlAttempt(
  session: AgentV2Session,
  target: ConversationState['responseBinding'],
  command: V2ResponseControlCommand,
  key: string,
  signal: AbortSignal,
  allowRevisionRetry: boolean,
): Promise<V2ResponseControl> {
  await session.authorize(signal);
  if (target === undefined) throw new Error('This turn does not support response controls.');
  const retained = session.journal.readControl();
  let intent: ControlRecord;
  if (retained?.commandId === null) {
    if (
      retained.responseId !== target.responseId ||
      retained.body.turn_sequence !== target.turnSequence ||
      JSON.stringify(retained.body.command) !== JSON.stringify(command)
    )
      throw new Error('Recover your pending action before requesting a different action.');
    intent = retained;
  } else {
    const observed = await session.responses.observeControls(
      session.journal.agentId,
      target.responseId,
      { signal },
    );
    if (observed.target?.turn_sequence !== target.turnSequence || !observed.target_active)
      throw new Error(
        'This response is no longer active. Its recorded outcome is available in the conversation.',
      );
    intent = session.journal.retainControl({
      key,
      responseId: target.responseId,
      commandId: null,
      body: {
        turn_sequence: target.turnSequence,
        expected_agent_revision: observed.agent_revision,
        command,
      },
    });
  }
  try {
    return await send(session, intent, signal, false);
  } catch (error) {
    if (
      allowRevisionRetry &&
      error instanceof ApiError &&
      error.status === 409 &&
      error.code === 'response_control_not_admitted'
    ) {
      session.journal.refusedControl(intent.key);
      return submitV2ControlAttempt(session, target, command, retryKey(), signal, false);
    }
    if (isDefinitiveRefusal(error)) session.journal.refusedControl(intent.key);
    throw error;
  }
}

export async function recoverV2Control(
  session: AgentV2Session,
  signal: AbortSignal,
): Promise<void> {
  await session.authorize(signal);
  const intent = session.journal.readControl();
  if (intent !== null && intent.commandId === null) await send(session, intent, signal);
}

function retryKey(): string {
  return typeof crypto.randomUUID === 'function'
    ? crypto.randomUUID()
    : `control-retry-${String(Date.now())}-${String(Math.random()).slice(2)}`;
}

function isDefinitiveRefusal(error: unknown): error is ApiError {
  return (
    error instanceof ApiError &&
    ((error.status === 409 && error.code === 'response_control_not_admitted') ||
      (error.status === 422 && error.code === 'invalid_control_request'))
  );
}

async function send(
  session: AgentV2Session,
  intent: ControlRecord,
  signal: AbortSignal,
  clearDefinitiveRefusal = true,
) {
  await session.authorize(signal);
  let accepted;
  try {
    accepted = await session.responses.submitControl(
      session.journal.agentId,
      intent.responseId,
      intent.body,
      { idempotencyKey: intent.key, signal },
    );
  } catch (error) {
    // Generic conflicts can hide an already-admitted receipt behind a fresh
    // authority/revision fence, including browser transport retries. Only a
    // definitive no-admission code or invalid body permits replacement.
    if (clearDefinitiveRefusal && isDefinitiveRefusal(error))
      session.journal.refusedControl(intent.key);
    throw error;
  }
  await session.authorize(signal);
  session.journal.acceptedControl(intent.key, accepted.control.receipt.command_id);
  // Admission or delivery does not end the displayed turn. Only durable replay does.
  return accepted;
}
