import { rateLimitDelay } from './readRetry';
import { useEffect, useState } from 'react';
import { commandIsTerminal, type V2CommandStatus } from '@cma/client';
import type { AgentV2Session } from './client';
import { transientReadFailure } from '../loadAgent';
import { observeRead } from './readObserver';
import type { ConversationState } from '../model';

export type StopRequestState = 'stopping' | 'checking-request';

function notice(command: V2CommandStatus, interruption: boolean): string | undefined {
  // Successful delivery is reflected by the conversation; keep only actionable status.
  if (command.status === 'completed') return undefined;
  if (interruption) {
    return command.status === 'failed' || command.status === 'cancelled'
      ? 'The current turn could not be stopped. Your new message is saved; check the conversation before retrying.'
      : 'Stopping the current turn…';
  }
  if (command.status === 'failed' || command.status === 'cancelled')
    return 'Your response action could not be delivered. Review the conversation before requesting another action.';
  return command.status === 'queued'
    ? 'Your response action is queued.'
    : 'Your response action is being delivered.';
}

function retainedAction(session: AgentV2Session | undefined) {
  const control = session?.journal.readControl();
  const interruption = session?.journal.interruption.read();
  // A later turn's Stop/approval supersedes an older interruption receipt. Two
  // cancellation requests for the same turn share the same runtime outcome.
  if (
    interruption?.receipt &&
    (!control?.commandId || interruption.body.turn_sequence >= control.body.turn_sequence)
  )
    return {
      commandId: interruption.receipt.controlId,
      turnSequence: interruption.body.turn_sequence,
      interruption: true,
    };
  return control?.commandId
    ? {
        commandId: control.commandId,
        turnSequence: control.body.turn_sequence,
        interruption: false,
      }
    : undefined;
}

/** Observe the retained receipt; GET retries never submit or retarget an action. */
export function useControlStatus(
  session: AgentV2Session | undefined,
  binding: ConversationState['responseBinding'],
  onIdentityChange: (error: unknown) => boolean,
) {
  const [result, setResult] = useState<{
    session: AgentV2Session;
    id: string;
    text?: string;
    failed?: boolean;
  }>();
  let commandId: string | undefined;
  let stopRequest: StopRequestState | undefined;
  try {
    commandId = retainedAction(session)?.commandId;
    const control = session?.journal.readControl();
    if (
      binding &&
      control?.body.command.type === 'cancel' &&
      control.responseId === binding.responseId &&
      control.body.turn_sequence === binding.turnSequence
    ) {
      if (control.commandId === null) stopRequest = 'checking-request';
      else if (!(result?.session === session && result?.id === control.commandId && result.failed))
        stopRequest = 'stopping';
    }
  } catch {
    // Mutation and observation paths clear views whose local epoch has expired.
  }
  useEffect(() => {
    if (session === undefined || commandId === undefined) return;
    let retry = 1000;
    let failures = 0;
    let previous: string | undefined;
    const observer = observeRead(async (signal) => {
      try {
        await session.authorize(signal);
        const command = await session.responses.readCommand(commandId, { signal });
        await session.authorize(signal);
        const intent = retainedAction(session);
        if (intent?.commandId !== commandId) return;
        if (
          command.receipt.agent_id !== session.journal.agentId ||
          command.receipt.response_id !== null ||
          (command.turn_sequence !== null && command.turn_sequence !== intent.turnSequence)
        )
          throw new Error('The action receipt changed its conversation target.');
        setResult({
          session,
          id: commandId,
          text: notice(command, intent.interruption),
          failed: command.status === 'failed' || command.status === 'cancelled',
        });
        if (commandIsTerminal(command)) return;
        failures = 0;
        const progress = JSON.stringify([
          command.status,
          command.delivery,
          command.observed_cursor,
          command.turn_sequence,
        ]);
        retry = previous === progress ? Math.min(retry * 2, 10000) : 1000;
        previous = progress;
        return { next: retry, cooldown: retry };
      } catch (error) {
        if (signal.aborted || onIdentityChange(error)) return;
        setResult({
          session,
          id: commandId,
          text: 'Your response action status is unavailable. Its original receipt is retained.',
        });
        if (!transientReadFailure(error)) return;
        previous = undefined;
        retry = Math.min(retry * 2, 10000);
        const next = rateLimitDelay(error, retry, ++failures);
        return { next, cooldown: next, failed: true };
      }
    });
    return () => observer.dispose();
  }, [session, commandId, onIdentityChange]);
  return {
    notice: result?.session === session && result?.id === commandId ? result?.text : undefined,
    stopRequest,
  };
}
