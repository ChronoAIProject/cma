import { useRetainedOutput, type RetainedOutputAccess } from './useRetainedOutput';
import { useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import type { TaskAcceptance } from '@cma/client';
import type { ApprovalDecision } from './controller';
import type { ConversationState } from './conversation';
import { StreamEntry } from './StreamEntry';
import { createStreamTimeline } from './streamTimeline';
import { ToolCallGroup } from './ToolCallRows';
import type { OptimisticMessage } from './optimisticMessages';
import type { AgentViewPosition } from './viewPosition';
import { UserMessage } from './UserMessage';
import { MessageAvatar, messageIdentity, type MessageUser } from './MessageIdentity';
import { useMessageArrival } from './messageArrival';
import { ActiveThinking } from './ActiveThinking';
import { MessageFooter } from './MessageFooter';
import type { MessageFeedbackControls } from './useMessageFeedback';
import { Button } from '../ui/Button';
import { MessageMediaContext, type MessageMediaAccess } from './mediaSources';
import { AnswerDeliveryContext, type AnswerDeliveryAccess } from './answerDeliveries';
import { ConversationEmpty } from './ConversationEmpty';
import type { ResourceStatus } from '../character/status';
import { messageStreamKey } from './streamTimeline';
import { acceptanceForTurn } from './taskAcceptance';
import { currentExternalApproval } from './externalApproval';
import { TurnRetryAction } from './TurnRetryAction';
import { TurnMascotContext, useTurnVisual, type TurnPresentation } from './turnVisual';

interface MessageStreamProps {
  readonly retained?: boolean;
  readonly conversation: ConversationState;
  readonly taskAcceptance?: TaskAcceptance;
  readonly onTaskDeliveryConfirmed?: () => void;
  readonly retainedOutput?: RetainedOutputAccess;
  readonly approvalBusy?: boolean;
  readonly approvalReadOnly?: boolean;
  readonly onApproval: (id: string, decision: ApprovalDecision) => void;
  readonly onReviewRetry?: (input: string) => void;
  readonly children?: ReactNode;
  readonly emptyMessage?: string;
  /** The Agent's recorded status, which poses the empty conversation's character. */
  readonly agentStatus?: ResourceStatus;
  readonly optimisticMessages?: readonly OptimisticMessage[];
  readonly viewPosition?: AgentViewPosition;
  readonly user?: MessageUser;
  readonly unknownUserName?: string;
  readonly arrivalsEnabled?: boolean;
  readonly feedback?: MessageFeedbackControls;
  readonly media?: MessageMediaAccess;
  readonly deliverables?: AnswerDeliveryAccess;
  /** Local interrupt admission is pending; terminal state still comes from the event stream. */
  readonly stopping?: boolean;
  readonly turnPresentation?: TurnPresentation;
}

export function MessageStream({
  retained: historyOnly = false,
  conversation,
  taskAcceptance,
  onTaskDeliveryConfirmed,
  retainedOutput,
  approvalBusy = false,
  approvalReadOnly = false,
  onApproval,
  onReviewRetry,
  children,
  emptyMessage = 'Tell the Agent what you need in your own words, for example “Compare three project management tools for a small team.”',
  agentStatus,
  optimisticMessages,
  viewPosition,
  user,
  unknownUserName,
  arrivalsEnabled = true,
  feedback,
  media,
  deliverables,
  stopping = false,
  turnPresentation,
}: MessageStreamProps) {
  const activeRunId = historyOnly ? undefined : conversation.activeRunId;
  const latestTurnId = conversation.latestTurnId;
  const turnCompleted =
    activeRunId === undefined &&
    latestTurnId !== undefined &&
    conversation.turnOutcomes?.[latestTurnId] === 'completed';
  const {
    client: turnClient,
    agentId: turnAgent,
    viewerId: turnViewer,
    authority: turnAccess,
  } = retainedOutput ?? {};
  const epoch = turnPresentation?.authEpoch;
  const turnAuthority = useMemo(
    () =>
      turnClient && epoch !== undefined
        ? { turnClient, turnAgent, turnViewer, turnAccess, epoch }
        : undefined,
    [turnClient, turnAgent, turnViewer, turnAccess, epoch],
  );
  const turnVisual = useTurnVisual(
    historyOnly ? undefined : turnPresentation,
    turnAuthority,
    conversation,
    taskAcceptance,
  );
  const {
    agentId: deliveryAgent,
    viewerId,
    client: deliveryClient,
    authority,
  } = deliverables ?? {};
  const deliveryAccess = useMemo(
    () =>
      deliveryAgent && viewerId && deliveryClient
        ? { agentId: deliveryAgent, viewerId, client: deliveryClient, authority }
        : undefined,
    [deliveryAgent, viewerId, deliveryClient, authority],
  );
  const [projectTimeline] = useState(createStreamTimeline);
  const retained = useRetainedOutput(retainedOutput, conversation);
  const { items, continued, previousRole } = projectTimeline(retained.conversation);
  const waitingForApproval = currentExternalApproval(conversation) !== undefined;
  const liveNotices = conversation.recovery.filter((notice) => notice.clearsOn !== undefined);
  const verdictOnPlan =
    taskAcceptance !== undefined &&
    items.some(
      (entry) => entry.kind === 'plan' && acceptanceForTurn(taskAcceptance, entry.item.runId),
    );
  const { agentId, files, workingDirectory } = media ?? {};
  const mediaAccess = useMemo(
    () =>
      agentId === undefined || files === undefined
        ? undefined
        : { agentId, files, workingDirectory },
    [agentId, files, workingDirectory],
  );
  const streamRef = useMessageArrival(conversation, optimisticMessages, arrivalsEnabled);
  const identity = messageIdentity('user', user, unknownUserName);
  const endRef = useRef<HTMLDivElement>(null);
  const pinned = useRef(viewPosition?.pinned ?? true);
  const previousTop = useRef(viewPosition?.top ?? 0);

  useLayoutEffect(() => {
    const scroller = endRef.current?.closest('.agent-surface__conversation');
    if (!(scroller instanceof HTMLElement)) return;
    if (viewPosition !== undefined) scroller.scrollTop = viewPosition.top;
    const track = () => {
      const atEnd = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight <= 48;
      if (atEnd) pinned.current = true;
      else if (scroller.scrollTop < previousTop.current) pinned.current = false;
      previousTop.current = scroller.scrollTop;
      if (viewPosition !== undefined) {
        viewPosition.top = scroller.scrollTop;
        viewPosition.pinned = pinned.current;
      }
    };
    const follow = () => {
      if (!pinned.current) return;
      scroller.scrollTop = scroller.scrollHeight;
      previousTop.current = scroller.scrollTop;
    };
    scroller.addEventListener('scroll', track, { passive: true });
    const observer = typeof ResizeObserver === 'undefined' ? undefined : new ResizeObserver(follow);
    observer?.observe(scroller);
    for (const child of scroller.children) observer?.observe(child);
    return () => {
      if (viewPosition !== undefined) {
        viewPosition.top = scroller.scrollTop;
        viewPosition.pinned = pinned.current;
      }
      scroller.removeEventListener('scroll', track);
      observer?.disconnect();
    };
  }, [viewPosition]);

  useLayoutEffect(() => {
    const scroller = endRef.current?.closest('.agent-surface__conversation');
    if (pinned.current && scroller instanceof HTMLElement) {
      scroller.scrollTop = scroller.scrollHeight;
      previousTop.current = scroller.scrollTop;
    }
  }, [conversation, optimisticMessages]);

  return (
    <MessageResources media={mediaAccess} deliverables={deliveryAccess}>
      <div className="message-stream" aria-live="polite" ref={streamRef}>
        {items.length === 0 && !optimisticMessages?.length && activeRunId === undefined ? (
          <ConversationEmpty message={emptyMessage} status={agentStatus} />
        ) : null}
        {items.map((entry) =>
          entry.kind === 'tools' || entry.kind === 'tool' ? (
            <ToolCallGroup
              key={entry.kind === 'tools' ? entry.id : `tools:${entry.item.id}`}
              group={
                entry.kind === 'tools'
                  ? entry
                  : {
                      kind: 'tools',
                      id: `tools:${entry.item.id}`,
                      items: [entry.item],
                      failed: entry.item.status === 'failed',
                    }
              }
              activeRunId={activeRunId}
            />
          ) : (
            <StreamEntry
              retained={historyOnly}
              key={messageStreamKey(entry)}
              entry={entry}
              approvalBusy={approvalBusy}
              hideApprovalCharacter={turnAuthority !== undefined}
              approvalReadOnly={
                approvalReadOnly ||
                (entry.kind === 'approval' &&
                  entry.item.source === 'external' &&
                  conversation.externalApprovalProjection?.reviewOrigin !== undefined &&
                  !conversation.externalApprovalProjection.canReview)
              }
              onApproval={onApproval}
              activeRunId={activeRunId}
              approvalActive={
                entry.kind === 'approval' &&
                entry.item.source === 'external' &&
                entry.item.runId === activeRunId &&
                conversation.turnOutcomes?.[entry.item.runId] === undefined
              }
              approvalReviewPolicy={conversation.externalApprovalReviewPolicy}
              recoveryAction={
                entry.kind === 'recovery' ? (
                  <TurnRetryAction
                    conversation={conversation}
                    notice={entry.item}
                    onPrepare={approvalReadOnly ? undefined : onReviewRetry}
                  />
                ) : undefined
              }
              taskAcceptance={
                entry.kind === 'message' || entry.kind === 'plan'
                  ? acceptanceForTurn(taskAcceptance, entry.item.runId)
                  : undefined
              }
              verdictOnAnswer={!verdictOnPlan}
              onTaskDeliveryConfirmed={onTaskDeliveryConfirmed}
              planOutcome={
                entry.kind === 'plan' && entry.item.runId !== undefined
                  ? conversation.turnOutcomes?.[entry.item.runId]
                  : undefined
              }
              user={user}
              unknownUserName={unknownUserName}
              feedback={feedback}
              continued={entry.kind === 'message' && continued.has(entry.item.id)}
            />
          ),
        )}
        <div className="message-stream__live-recovery-slot" aria-live="polite">
          {(waitingForApproval ? liveNotices.slice(-1) : liveNotices).map((notice) => (
            <p className="message__delivery" role="status" key={notice.id}>
              {notice.title}
            </p>
          ))}
        </div>
        {historyOnly ? null : (
          <ActiveThinking conversation={conversation} stopping={stopping} brand={turnVisual} />
        )}
        {optimisticMessages?.map((message, index) => (
          <article
            className={`message message--user${message.queued ? ' message--queued' : ''}`}
            aria-label={identity.name}
            key={`local:${message.id}`}
            data-message-key={`local:${message.id}`}
          >
            <MessageAvatar identity={identity} continued={index > 0 || previousRole === 'user'} />
            <div className="message__body">
              {index > 0 || previousRole === 'user' ? null : (
                <span className="message__role">{identity.name}</span>
              )}
              <UserMessage content={message.content} />
              <span className="message__delivery" role="status">
                {message.state === 'sending'
                  ? 'Sending…'
                  : message.state === 'accepted'
                    ? message.queued
                      ? 'Queued'
                      : 'Sent'
                    : message.state === 'unconfirmed'
                      ? 'Awaiting confirmation'
                      : 'Not sent'}
              </span>
              <MessageFooter
                id={message.id}
                canVote={false}
                at={message.at}
                content={message.content}
                committed={false}
              />
            </div>
          </article>
        ))}
        {retained.error ? (
          <div className="message-footer__error" role="status">
            <span>{retained.error}</span>
            <Button size="sm" variant="ghost" onClick={retained.retry}>
              Retry retained output
            </Button>
          </div>
        ) : null}
        <TurnMascotContext value={turnVisual !== undefined}>{children}</TurnMascotContext>
        {feedback?.error ? (
          <div className="message-footer__error" role="status">
            <span>{feedback.error}</span>
            <Button size="sm" onClick={feedback.reload}>
              Retry message feedback
            </Button>
          </div>
        ) : null}
        <div ref={endRef} />
      </div>
      {historyOnly ? null : (
        <span className="sr-only" role="status" aria-atomic="true">
          {turnCompleted ? 'Turn completed.' : null}
        </span>
      )}
    </MessageResources>
  );
}

function MessageResources({
  children,
  media,
  deliverables,
}: {
  readonly children: ReactNode;
  readonly media?: MessageMediaAccess;
  readonly deliverables?: AnswerDeliveryAccess;
}) {
  return (
    <AnswerDeliveryContext.Provider value={deliverables}>
      <MessageMediaContext.Provider value={media}>{children}</MessageMediaContext.Provider>
    </AnswerDeliveryContext.Provider>
  );
}
