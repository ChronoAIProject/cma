import type { ConversationState } from './model';
import { attachmentMarker } from './attachments';

/** Prepare a fresh draft only; dispatch remains the existing explicit Send action. */
export function turnRetryInput(
  state: ConversationState,
  runId: string,
): { input: string } | { unavailable: string } | undefined {
  if (
    state.activeRunId !== undefined ||
    state.latestTurnId !== runId ||
    state.turnOutcomes?.[runId] !== 'failed'
  )
    return undefined;
  if (
    !state.recovery.some(
      (notice) =>
        notice.runId === runId &&
        ['agent_turn_timeout', 'approval_timeout'].includes(
          notice.technicalDetails?.category ?? '',
        ),
    )
  )
    return undefined;
  const original = state.messages.find(
    (message) => message.runId === runId && message.role === 'user' && !message.liveOnly,
  );
  if (
    !original?.content.trim() ||
    original.status !== 'complete' ||
    original.sequence === undefined
  )
    return {
      unavailable:
        'The complete original message is unavailable. Review the conversation and write a new message to continue.',
    };
  if (original.content.includes(attachmentMarker))
    return {
      unavailable:
        'The original attachments need to be checked before retrying. Review the conversation and attach the files to a new message.',
    };
  return { input: original.content };
}
