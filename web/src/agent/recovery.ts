import { upsert } from './conversationValues';
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
