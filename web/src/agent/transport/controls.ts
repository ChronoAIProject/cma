import { ApiError, type V2ResponseControlCommand, type V2ResponseControl } from '@cma/client';
import type { ConversationState } from '../model';
import type { AgentV2Session } from './client';
import type { ControlRecord } from './controlRecord';
import type { RequestRecovery } from './requestRecovery';

export async function submitV2Control(
  session: AgentV2Session,
  target: ConversationState['responseBinding'],
  command: V2ResponseControlCommand,
  key: string,
  signal: AbortSignal,
): Promise<V2ResponseControl> {
  return session.withRecovery(
    signal,
    (recovery) => submitV2ControlAttempt(session, target, command, key, recovery, true),
    command.type === 'cancel',
  );
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
  recovery: RequestRecovery,
  allowRevisionRetry: boolean,
): Promise<V2ResponseControl> {
  const signal = recovery.signal;
  if (
    command.type !== 'cancel' ||
    target === undefined ||
    session.journal.readControl()?.commandId === null
  )
    await recovery.retry(() => authorize(session, command, signal));
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
    const observed = await recovery.retry(async (retry) => {
      if (command.type === 'cancel')
        return observeCancelTarget(session, target.responseId, recovery);
      if (retry > 0) await authorize(session, command, signal);
      return session.responses.observeControls(session.journal.agentId, target.responseId, {
        signal,
        timeoutMs: recovery.timeoutMs,
      });
    });
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
    return await send(session, intent, recovery, false);
  } catch (error) {
    if (
      allowRevisionRetry &&
      !recovery.retried &&
      error instanceof ApiError &&
      error.status === 409 &&
      error.code === 'response_control_not_admitted'
    ) {
      session.journal.refusedControl(intent.key);
      return submitV2ControlAttempt(session, target, command, retryKey(), recovery, false);
    }
    if (isDefinitiveRefusal(error)) session.journal.refusedControl(intent.key);
    throw error;
  }
}

async function observeCancelTarget(
  session: AgentV2Session,
  responseId: string,
  recovery: RequestRecovery,
) {
  const attempt = new AbortController();
  const abort = () => attempt.abort(recovery.signal.reason);
  recovery.signal.throwIfAborted();
  recovery.signal.addEventListener('abort', abort, { once: true });
  try {
    // Both reads are fresh. Keep the observation private until identity succeeds;
    // send() still fences the POST and its receipt with separate fresh checks.
    const [, observed] = await Promise.all([
      session.authorizeCancel(attempt.signal),
      Promise.resolve().then(() =>
        session.responses.observeControls(session.journal.agentId, responseId, {
          signal: attempt.signal,
          timeoutMs: recovery.timeoutMs,
        }),
      ),
    ]);
    attempt.signal.throwIfAborted();
    session.journal.read();
    return observed;
  } finally {
    recovery.signal.removeEventListener('abort', abort);
    // A failed read cancels its sibling; Promise.all already handles both errors.
    attempt.abort();
  }
}

export async function recoverV2Control(
  session: AgentV2Session,
  signal: AbortSignal,
): Promise<void> {
  const cancel = session.journal.readControl()?.body.command.type === 'cancel';
  return session.withRecovery(
    signal,
    async (recovery) => {
      const recoverySignal = recovery.signal;
      recoverySignal.throwIfAborted();
      // send() checks fresh authority before dispatch and again after receipt.
      // Only guarded local reads precede it: a second adjacent network check
      // delays recovery without protecting an additional IO boundary.
      const intent = session.journal.readControl();
      if (intent !== null && intent.commandId === null) await send(session, intent, recovery);
    },
    cancel,
  );
}

function retryKey(): string {
  return typeof crypto.randomUUID === 'function'
    ? crypto.randomUUID()
    : `control-retry-${String(Date.now())}-${String(Math.random()).slice(2)}`;
}

function authorize(
  session: AgentV2Session,
  command: V2ResponseControlCommand | undefined,
  signal: AbortSignal,
) {
  return command?.type === 'cancel' ? session.authorizeCancel(signal) : session.authorize(signal);
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
  recovery: RequestRecovery,
  clearDefinitiveRefusal = true,
) {
  const signal = recovery.signal;
  let accepted: V2ResponseControl | undefined;
  const requireRetained = () => {
    const retained = session.journal.readControl();
    if (
      retained?.key !== intent.key ||
      retained.responseId !== intent.responseId ||
      JSON.stringify(retained.body) !== JSON.stringify(intent.body)
    )
      throw new Error('The retained control changed while its receipt was being recovered.');
  };
  const sendAttempt = async () => {
    if (accepted === undefined) {
      await authorize(session, intent.body.command, signal);
      requireRetained();
      try {
        accepted = await session.responses.submitControl(
          session.journal.agentId,
          intent.responseId,
          intent.body,
          { idempotencyKey: intent.key, signal, timeoutMs: recovery.timeoutMs },
        );
      } catch (error) {
        // Generic conflicts can hide an already-admitted receipt behind a fresh
        // authority/revision fence, including browser transport retries. Only a
        // definitive no-admission code or invalid body permits replacement.
        if (clearDefinitiveRefusal && isDefinitiveRefusal(error))
          session.journal.refusedControl(intent.key);
        throw error;
      }
    }
    await authorize(session, intent.body.command, signal);
    requireRetained();
    session.journal.acceptedControl(intent.key, accepted.control.receipt.command_id);
    // Admission or delivery does not end the displayed turn. Only durable replay does.
    return accepted;
  };
  return recovery.retry(
    sendAttempt,
    (error) =>
      intent.body.command.type === 'cancel' &&
      error instanceof ApiError &&
      error.status === 503 &&
      error.code === 'response_control_busy',
  );
}
