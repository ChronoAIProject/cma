import type { ConversationState, RecoveryNotice } from './model';

/** A terminal ends live recovery, but cannot make an unrecorded fragment complete. */
export function settleLiveRecovery(state: ConversationState): ConversationState {
  const ended = (runId: string | undefined) =>
    runId !== undefined && state.turnOutcomes?.[runId] !== undefined;
  const partialRuns = new Set(
    state.messages
      .filter((message) => message.liveOnly && ended(message.runId))
      .map((message) => message.runId!),
  );
  const recovery: RecoveryNotice[] = state.recovery.filter((notice) => {
    if (notice.id.startsWith('partial-output:')) return partialRuns.has(notice.runId ?? '');
    return (
      notice.clearsOn === undefined ||
      !(
        ended(notice.runId) ||
        (notice.runId === undefined && state.activeRunId === undefined && ended(state.latestTurnId))
      )
    );
  });
  for (const runId of partialRuns) {
    const id = `partial-output:${runId}`;
    if (recovery.some((notice) => notice.id === id)) continue;
    recovery.push({
      id,
      runId,
      afterSequence: state.cursor,
      title: 'Incomplete live output',
      detail:
        'This turn ended before all visible reply text was recorded. The partial text is retained here and may be incomplete.',
    });
  }
  return {
    ...state,
    recovery,
    messages: state.messages.map((message) =>
      message.liveOnly && ended(message.runId) && message.status === 'streaming'
        ? { ...message, status: 'error' }
        : message,
    ),
  };
}
