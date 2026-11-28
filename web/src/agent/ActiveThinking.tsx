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
import { AgentMascot } from '../brand/AgentMascot';
import type { TurnVisual } from './turnVisual';

/** Ephemeral turn status follows the reply in the conversation, never in stored history. */
export function ActiveThinking({
  conversation,
  stopping = false,
  brand,
}: {
  readonly conversation: ConversationState;
  /** A local admission request is in flight; the durable turn is still authoritative. */
  readonly stopping?: boolean;
  readonly brand?: TurnVisual;
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
  const externalApproval = currentExternalApproval(conversation);
  if (externalApproval && !brand) return stopping ? <div role="status">Stopping…</div> : null;
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
  const thinking = (
    <Presence>
      {runId === undefined ? null : (
        <div className="active-thinking" ref={placement} aria-live="off">
          <div className="active-thinking__content">
            <Thinking
              key={runId}
              branded={brand !== undefined}
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
  const announce = runId !== undefined && !externalApproval && brand?.visual.action !== 'question';
  return (
    <div className={brand ? 'turn-activity' : undefined}>
      {brand ? (
        <div
          aria-live={announce ? undefined : 'off'}
          role={announce ? 'status' : undefined}
          title={brand.visual.detail}
        >
          <AgentMascot visual={brand.visual} playback={brand.playback} size={48} />
        </div>
      ) : null}
      {externalApproval ? stopping ? <div role="status">Stopping…</div> : null : thinking}
    </div>
  );
}
