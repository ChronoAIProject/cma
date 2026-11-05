import type { ConversationState } from './model';

/** Loss evidence belongs to the turn even after its display notice is evicted. */
export function hasRecordedGap(state: ConversationState, runId: string | undefined): boolean {
  return (
    (runId !== undefined && (state.incompleteRuns ?? []).includes(runId)) ||
    state.recovery.some(
      (notice) =>
        notice.runId === runId &&
        (notice.id.startsWith('gap:') || notice.id.startsWith('history-gap:')),
    )
  );
}

export function recordedGapRuns(state: ConversationState, kind: string) {
  return (kind === 'history_gap' || kind === 'conversation_gap') && state.activeRunId !== undefined
    ? [...new Set([...(state.incompleteRuns ?? []), state.activeRunId])]
    : state.incompleteRuns;
}
