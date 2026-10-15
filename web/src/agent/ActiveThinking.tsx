import type { ConversationState } from './model';
import { Thinking } from './Thinking';
import { isPendingTool } from './toolStatus';
import { visibleTurnText } from './tokenEstimate';

/** This dock is outside the history scrollport so it remains visible while reading. */
export function ActiveThinking({ conversation }: { readonly conversation: ConversationState }) {
  const runId = conversation.activeRunId;
  if (runId === undefined) return null;
  return (
    <div className="agent-surface__thinking">
      <Thinking
        key={runId}
        startedAt={conversation.activeRunStartedAt}
        visibleText={visibleTurnText(conversation)}
        visibleAnchor={
          String(conversation.visibleUsageAnchor?.turnSequence) === runId
            ? conversation.visibleUsageAnchor?.parts
            : undefined
        }
        usage={
          String(conversation.usage?.currentTurn?.turnSequence) === runId
            ? conversation.usage?.currentTurn
            : undefined
        }
        text={conversation.reasoning
          .filter((item) => item.runId === runId)
          .map((item) => item.content)
          .join('\n')}
        activities={conversation.toolCalls
          .filter((item) => item.runId === runId && isPendingTool(item))
          .map((item) => ({
            id: item.id,
            kind: item.kind ?? 'Tool',
            target: item.title,
            status: item.status,
            detail: item.detail === undefined ? undefined : { text: item.detail, truncated: false },
          }))}
      />
    </div>
  );
}
