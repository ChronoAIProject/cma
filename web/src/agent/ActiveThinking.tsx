import type { ConversationState } from './model';
import { Thinking } from './Thinking';
import { isPendingTool } from './toolStatus';
import { visibleTurnText } from './tokenEstimate';
import { Presence } from '../ui/Presence';
import { useThinkingPlacement } from './thinkingPlacement';
import { characterForTurn } from '../character/status';

/** Ephemeral turn status follows the reply in the conversation, never in stored history. */
export function ActiveThinking({ conversation }: { readonly conversation: ConversationState }) {
  const runId = conversation.activeRunId;
  const placement = useThinkingPlacement(conversation);
  const visibleText = visibleTurnText(conversation);
  const activities = conversation.toolCalls.filter(
    (item) => item.runId === runId && isPendingTool(item),
  );
  const pose = characterForTurn({
    pendingTools: activities.length,
    replying: visibleText.some((part) => part.id === 'reply' && part.text !== ''),
  });
  return (
    <Presence>
      {runId === undefined ? null : (
        <div className="active-thinking" ref={placement} aria-live="off">
          <div className="active-thinking__content">
            <Thinking
              key={runId}
              startedAt={conversation.activeRunStartedAt}
              pose={pose}
              visibleText={visibleText}
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
              activities={activities.map((item) => ({
                id: item.id,
                kind: item.kind ?? 'Tool',
                target: item.title,
                status: item.status,
                detail:
                  item.detail === undefined ? undefined : { text: item.detail, truncated: false },
              }))}
            />
          </div>
        </div>
      )}
    </Presence>
  );
}
