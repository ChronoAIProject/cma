import { useLayoutEffect, useRef } from 'react';
import type { ReactNode } from 'react';
import type { ApprovalDecision } from './controller';
import type { ConversationState } from './conversation';
import { StreamEntry } from './StreamEntry';
import { groupToolCalls, type Entry } from './timeline';
import { ToolCallGroup } from './ToolCallRows';
import { isPendingTool } from './toolStatus';
import type { OptimisticMessage } from './optimisticMessages';
import type { AgentViewPosition } from './viewPosition';
import { UserMessage } from './UserMessage';
import { MessageAvatar, messageIdentity, type MessageUser } from './MessageIdentity';
import { useMessageArrival } from './messageArrival';
import { ActiveThinking } from './ActiveThinking';
import { MessageFooter } from './MessageFooter';
import type { MessageFeedbackControls } from './useMessageFeedback';
import { Button } from '../ui/Button';

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
}

export function MessageStream({
  conversation,
  approvalBusy = false,
  approvalReadOnly = false,
  onApproval,
  children,
  emptyMessage = 'Start the conversation.',
  optimisticMessages,
  viewPosition,
  user,
  unknownUserName,
  arrivalsEnabled = true,
  feedback,
}: MessageStreamProps) {
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

  const entries: Entry[] = [
    ...conversation.messages.map((item) => ({ kind: 'message' as const, item })),
    ...conversation.toolCalls
      .filter(
        (item) =>
          !isPendingTool(item) ||
          item.runId !== conversation.activeRunId ||
          conversation.activeRunId === undefined,
      )
      .map((item) => ({ kind: 'tool' as const, item })),
    ...conversation.approvals.map((item) => ({ kind: 'approval' as const, item })),
    ...conversation.recovery.map((item) => ({ kind: 'recovery' as const, item })),
  ];
  entries.sort((left, right) => (left.item.order ?? 0) - (right.item.order ?? 0));
  const finalReply = new Map<string, string>();
  for (const entry of entries)
    if (
      entry.kind === 'message' &&
      entry.item.role === 'assistant' &&
      entry.item.runId !== undefined &&
      !entry.item.liveOnly
    )
      finalReply.set(entry.item.runId, entry.item.id);

  return (
    <div className="message-stream" aria-live="polite" ref={streamRef}>
      {entries.length === 0 &&
      !optimisticMessages?.length &&
      conversation.activeRunId === undefined ? (
        <p className="message-stream__empty">{emptyMessage}</p>
      ) : null}
      {groupToolCalls(entries).map((entry) =>
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
      {optimisticMessages?.map((message) => (
        <article
          className={`message message--user${message.queued ? ' message--queued' : ''}`}
          aria-label={identity.name}
          key={`local:${message.id}`}
          data-message-key={`local:${message.id}`}
        >
          <MessageAvatar identity={identity} />
          <div className="message__body">
            <span className="message__role">{identity.name}</span>
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
  );
}
