import { ApiError } from '@cma/client';
import { useEffect, useState } from 'react';
import type { ResponseControlAvailability } from '../controlReadiness';
import type { AgentResourceState } from '../controllerTypes';
import { transientReadFailure } from '../loadAgent';
import type { AgentV2Session } from './client';
import { observeRead } from './readObserver';
import { retrySchedule } from './readRetry';

/** Observe the exact inline first reply without blocking history or issuing a command. */
export function useFirstReplyStop(
  state: AgentResourceState,
  availability: ResponseControlAvailability,
): ResponseControlAvailability {
  const first = state.status === 'ready' ? state.detail.firstMessage : undefined;
  const turn = state.status === 'ready' ? state.conversation.responseBinding : undefined;
  const session = state.status === 'ready' ? state.v2 : undefined;
  const isFirstReply =
    first?.purpose !== undefined &&
    turn !== undefined &&
    (first.turnSequence !== undefined
      ? first.turnSequence === turn.turnSequence
      : first.state === 'queued' ||
        first.state === 'dispatch_fenced' ||
        first.state === 'write_confirmed' ||
        first.state === 'execution_observed');
  const responseId = availability === 'available' && isFirstReply ? turn.responseId : undefined;
  const turnSequence = responseId === undefined ? undefined : turn?.turnSequence;
  const [read, setRead] = useState<{
    session: AgentV2Session;
    responseId: string;
    turnSequence: number;
    availability: ResponseControlAvailability;
  }>();

  useEffect(() => {
    if (!session || responseId === undefined || turnSequence === undefined) return;
    const publish = (availability: ResponseControlAvailability) =>
      setRead({ session, responseId, turnSequence, availability });
    let failures = 0;
    const observer = observeRead(async (signal) => {
      try {
        const observed = await session.responses.observeControls(
          session.journal.agentId,
          responseId,
          {
            signal,
          },
        );
        if (signal.aborted) return;
        if (!observed.target_active || observed.target?.turn_sequence !== turnSequence)
          publish('inactive');
        else publish(observed.runtime_support.cancel ? 'available' : 'unsupported');
      } catch (error) {
        if (signal.aborted) return;
        if (error instanceof ApiError && error.status === 403) publish('denied');
        else if (transientReadFailure(error)) return retrySchedule(error, ++failures);
        else publish('check-failed');
      }
    });
    return () => observer.dispose();
  }, [session, responseId, turnSequence]);

  if (!session || responseId === undefined) return availability;
  return read?.session === session &&
    read.responseId === responseId &&
    read.turnSequence === turnSequence
    ? read.availability
    : 'checking';
}
