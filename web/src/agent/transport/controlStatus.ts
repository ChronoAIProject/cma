import { useEffect, useState } from 'react';
import { commandIsTerminal, type V2CommandStatus } from '@chronoai/cma-sdk';
import type { AgentV2Session } from './client';
import { transientReadFailure } from '../loadAgent';

function notice(command: V2CommandStatus): string | undefined {
  // Successful delivery is reflected by the conversation; keep only actionable status.
  if (command.status === 'completed') return undefined;
  if (command.status === 'failed' || command.status === 'cancelled')
    return 'Your response action could not be delivered. Review the conversation before requesting another action.';
  return command.status === 'queued'
    ? 'Your response action is queued.'
    : 'Your response action is being delivered.';
}

/** Observe the retained receipt; GET retries never submit or retarget an action. */
export function useControlStatus(
  session: AgentV2Session | undefined,
  onIdentityChange: (error: unknown) => boolean,
): string | undefined {
  const [result, setResult] = useState<{ id: string; text: string | undefined }>();
  let commandId: string | undefined;
  try {
    commandId = session?.journal.readControl()?.commandId ?? undefined;
  } catch {
    // Mutation and observation paths clear views whose local epoch has expired.
  }
  useEffect(() => {
    if (session === undefined || commandId === undefined) return;
    const request = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let retry = 1000;
    const poll = async () => {
      try {
        await session.authorize(request.signal);
        const command = await session.responses.readCommand(commandId, { signal: request.signal });
        await session.authorize(request.signal);
        const intent = session.journal.readControl();
        if (intent?.commandId !== commandId) return;
        if (
          command.receipt.agent_id !== session.journal.agentId ||
          command.receipt.response_id !== null ||
          (command.turn_sequence !== null && command.turn_sequence !== intent.body.turn_sequence)
        )
          throw new Error('The action receipt changed its conversation target.');
        setResult({ id: commandId, text: notice(command) });
        if (commandIsTerminal(command)) return;
        retry = 1000;
      } catch (error) {
        if (request.signal.aborted || onIdentityChange(error)) return;
        setResult({
          id: commandId,
          text: 'Your response action status is unavailable. Its original receipt is retained.',
        });
        if (!transientReadFailure(error)) return;
        retry = Math.min(retry * 2, 10000);
      }
      timer = setTimeout(() => void poll(), retry);
    };
    void poll();
    return () => {
      request.abort();
      clearTimeout(timer);
    };
  }, [session, commandId, onIdentityChange]);
  return result?.id === commandId ? result?.text : undefined;
}
