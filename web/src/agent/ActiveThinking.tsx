import { elapsedTime, toolDuration } from './workspaceTime';
import type { ConversationState } from './model';
import { Thinking } from './Thinking';
import { isPendingTool } from './toolStatus';
import { Presence } from '../ui/Presence';
import { useThinkingPlacement } from './thinkingPlacement';

/** Ephemeral turn status follows the reply in the conversation, never in stored history. */
export function ActiveThinking({ conversation }: { readonly conversation: ConversationState }) {
  const runId = conversation.activeRunId;
  const placement = useThinkingPlacement(conversation);
  const activities = conversation.toolCalls.filter(
    (item) => item.runId === runId && isPendingTool(item),
  );
  const last = conversation.latestTurnId;
  const lastTiming = last === undefined ? undefined : conversation.turnTiming?.[last];
  const stoppedDuration = toolDuration(lastTiming?.startedAt, lastTiming?.finishedAt);
  if (runId === undefined && last && conversation.turnOutcomes?.[last] === 'interrupted') {
    return (
      <div className="workspace-stopped" role="status">
        <span aria-hidden="true">■</span>
        <span>
          {stoppedDuration === undefined
            ? 'Stopped'
            : `Stopped after ${elapsedTime(stoppedDuration)}`}
        </span>
      </div>
    );
  }
  return (
    <Presence>
      {runId === undefined ? null : (
        <div className="active-thinking" ref={placement} aria-live="off">
          <div className="active-thinking__content">
            <Thinking
              key={runId}
              startedAt={conversation.activeRunStartedAt}
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
