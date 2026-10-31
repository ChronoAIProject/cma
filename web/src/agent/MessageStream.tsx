import { useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { ReactNode } from 'react';
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
import { workedFor } from './turnTiming';
import { MessageMediaContext, type MessageMediaAccess } from './mediaSources';

interface MessageStreamProps {
  readonly conversation: ConversationState;
  readonly approvalBusy?: boolean;
  readonly approvalReadOnly?: boolean;
  readonly onApproval: (id: string, decision: ApprovalDecision) => void;
  readonly children?: ReactNode;
  readonly emptyMessage?: string;
  readonly optimisticMessages?: readonly OptimisticMessage[];
  readonly viewPosition?: AgentViewPosition;
  readonly user?: MessageUser;
  readonly unknownUserName?: string;
  readonly arrivalsEnabled?: boolean;
  readonly feedback?: MessageFeedbackControls;
  readonly media?: MessageMediaAccess;
}

export function MessageStream({
  conversation,
  approvalBusy = false,
  approvalReadOnly = false,
  onApproval,
  children,
  emptyMessage = 'Tell the Agent what you need in your own words, for example “Compare three project management tools for a small team.”',
  optimisticMessages,
  viewPosition,
  user,
  unknownUserName,
  arrivalsEnabled = true,
  feedback,
  media,
}: MessageStreamProps) {
  const [projectTimeline] = useState(createStreamTimeline);
  const { items, finalReply, continued, previousRole } = projectTimeline(conversation);
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
    <MessageMediaContext.Provider value={mediaAccess}>
      <div className="message-stream" aria-live="polite" ref={streamRef}>
        {items.length === 0 &&
        !optimisticMessages?.length &&
        conversation.activeRunId === undefined ? (
          <p className="message-stream__empty">{emptyMessage}</p>
        ) : null}
        {items.map((entry) =>
          entry.kind === 'tools' ? (
            <ToolCallGroup key={entry.id} group={entry} activeRunId={conversation.activeRunId} />
          ) : (
            <StreamEntry
              key={`${entry.kind}:${entry.item.id}`}
              entry={entry}
              approvalBusy={approvalBusy}
              approvalReadOnly={approvalReadOnly}
              onApproval={onApproval}
              activeRunId={conversation.activeRunId}
              user={user}
              unknownUserName={unknownUserName}
              feedback={feedback}
              continued={entry.kind === 'message' && continued.has(entry.item.id)}
              duration={
                entry.kind === 'message' &&
                entry.item.runId !== undefined &&
                entry.item.runId !== conversation.activeRunId &&
                finalReply.get(entry.item.runId) === entry.item.id
                  ? workedFor(conversation.turnTiming?.[entry.item.runId])
                  : undefined
              }
              showUsage={
                entry.kind === 'message' &&
                entry.item.runId !== undefined &&
                entry.item.runId !== conversation.activeRunId &&
                finalReply.get(entry.item.runId) === entry.item.id
              }
              usage={
                entry.kind === 'message' && entry.item.runId !== undefined
                  ? (conversation.turnUsage?.[entry.item.runId] ??
                    (String(conversation.usage?.currentTurn?.turnSequence) === entry.item.runId
                      ? conversation.usage?.currentTurn
                      : undefined))
                  : undefined
              }
            />
          ),
        )}
        <ActiveThinking conversation={conversation} />
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
        {children}
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
    </MessageMediaContext.Provider>
  );
}
