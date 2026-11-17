import type { AgentEvent } from '../api/types';
import { number, object, text, upsert } from './conversationValues';
import { clearLiveGapRun } from './liveMessage';
import { finishTurn } from './turnTiming';
import type { ConversationState, RecoveryNotice } from './model';

export function recover(state: ConversationState, notice: RecoveryNotice): ConversationState {
  return { ...state, recovery: upsert(state.recovery, notice) };
}

export function hasRecordedGap(state: ConversationState, runId: string | undefined): boolean {
  return state.recovery.some(
    (notice) =>
      notice.runId === runId &&
      (notice.id.startsWith('gap:') || notice.id.startsWith('history-gap:')),
  );
}

export function failedTurn(next: ConversationState, event: AgentEvent): ConversationState {
  const kind = event.kind;
  const failure = object(kind.failure);
  const runId = number(kind.turn_sequence)?.toString() ?? next.activeRunId;
  return recover(
    {
      ...finishTurn(
        next,
        number(kind.turn_sequence)?.toString() ?? next.activeRunId,
        event.appendedAt,
      ),
      liveGapRuns:
        runId === undefined ? next.liveGapRuns : clearLiveGapRun(next.liveGapRuns, runId),
      turnOutcomes: {
        ...next.turnOutcomes,
        [number(kind.turn_sequence)?.toString() ?? next.activeRunId ?? 'unattributed']: 'failed',
      },
      activeRunId: undefined,
      activeRunStartedAt: undefined,
      reasoning: [],
    },
    {
      id: `run-error:${String(event.seq)}`,
      runId:
        number(kind.turn_sequence) === undefined ? next.activeRunId : String(kind.turn_sequence),
      title: 'Agent run failed',
      detail: text(failure?.detail) ?? 'The Agent run did not complete.',
      code: text(failure?.class),
      retainedOutput: event.retainedOutput,
    },
  );
}
