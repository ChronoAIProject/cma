import type { ConversationState } from './model';

export function TurnCompletion({ conversation }: { readonly conversation: ConversationState }) {
  const latest = conversation.latestTurnId;
  const outcome = latest === undefined ? undefined : conversation.turnOutcomes?.[latest];
  const timing = latest === undefined ? undefined : conversation.turnTiming?.[latest];
  if (conversation.activeRunId !== undefined || outcome !== 'completed' || !timing?.finishedAt)
    return null;
  const elapsed = Date.parse(timing.finishedAt) - Date.parse(timing.startedAt);
  if (!Number.isFinite(elapsed) || elapsed < 0) return null;
  const seconds = Math.floor(elapsed / 1000);
  const minutes = Math.floor(seconds / 60);
  const remaining = seconds % 60;
  const duration =
    seconds === 0
      ? '<1s'
      : [...(minutes > 0 ? [`${minutes}m`] : []), ...(remaining > 0 ? [`${remaining}s`] : [])].join(
          ' ',
        );
  return (
    <span className="agent-toolbar__completion" role="status" aria-atomic="true">
      Done in {duration}
    </span>
  );
}
