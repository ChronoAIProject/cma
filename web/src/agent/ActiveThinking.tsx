import { elapsedTime, toolDuration } from './workspaceTime';
import type { ConversationState } from './model';
import { currentExternalApproval } from './externalApproval';
import { Thinking } from './Thinking';
import { isPendingTool } from './toolStatus';
import { Presence } from '../ui/Presence';
import { useThinkingPlacement } from './thinkingPlacement';
import { useMemo } from 'react';
import {
  thinkingProgressKey,
  recordedTurnActivity,
  hasVisibleTurnProgress,
} from './ThinkingProgress';
import { RESPONSE_STOP_COMMAND_NOTICE } from './stopAgentCopy';

/** Ephemeral turn status follows the reply in the conversation, never in stored history. */
export function ActiveThinking({
  conversation,
  stopping = false,
}: {
  readonly conversation: ConversationState;
  /** A local admission request is in flight; the durable turn is still authoritative. */
  readonly stopping?: boolean;
}) {
  const runId = conversation.activeRunId;
  const placement = useThinkingPlacement(conversation);
  const progressKey = useMemo(() => thinkingProgressKey(conversation), [conversation]);
  const activities = conversation.toolCalls.filter(
    (item) => item.runId === runId && isPendingTool(item),
  );
  const last = conversation.latestTurnId;
  const lastTiming = last === undefined ? undefined : conversation.turnTiming?.[last];
  const interruptedDuration = toolDuration(lastTiming?.startedAt, lastTiming?.finishedAt);
  if (currentExternalApproval(conversation))
    return stopping ? <div role="status">Stopping…</div> : null;
  if (runId === undefined && last && conversation.turnOutcomes?.[last] === 'interrupted') {
    return (
      <div className="workspace-stopped" role="status">
        <span aria-hidden="true">■</span>
        <span>
          {interruptedDuration === undefined
            ? 'Response interrupted.'
            : `Response interrupted after ${elapsedTime(interruptedDuration)}.`}{' '}
          {RESPONSE_STOP_COMMAND_NOTICE}
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
              progressKey={progressKey}
              waitingForResponse={!hasVisibleTurnProgress(conversation)}
              startedAt={conversation.activeRunStartedAt}
              recordedActivity={recordedTurnActivity(conversation)}
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
          {stopping ? (
            <div className="active-thinking__stopping" role="status">
              Stopping…
            </div>
          ) : null}
        </div>
      )}
    </Presence>
  );
}
