import { useLayoutEffect, useRef } from 'react';
import type { ReactNode } from 'react';
import type { ApprovalDecision } from './controller';
import type { ConversationState } from './conversation';
import { StreamEntry, type Entry } from './StreamEntry';
import { isPendingTool } from './toolStatus';
import type { OptimisticMessage } from './optimisticMessages';

interface MessageStreamProps {
  readonly conversation: ConversationState;
  readonly approvalBusy?: boolean;
  readonly approvalReadOnly?: boolean;
  readonly onApproval: (id: string, decision: ApprovalDecision) => void;
  readonly children?: ReactNode;
  readonly emptyMessage?: string;
  readonly optimisticMessages?: readonly OptimisticMessage[];
}

export function MessageStream({
  conversation,
  approvalBusy = false,
  approvalReadOnly = false,
  onApproval,
  children,
  emptyMessage = 'Start the conversation.',
  optimisticMessages,
}: MessageStreamProps) {
  const endRef = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);
  const previousTop = useRef(0);

  useLayoutEffect(() => {
    const scroller = endRef.current?.closest('.agent-surface__conversation');
    if (!(scroller instanceof HTMLElement)) return;
    const track = () => {
      const atEnd = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight <= 48;
      if (atEnd) pinned.current = true;
      else if (scroller.scrollTop < previousTop.current) pinned.current = false;
      previousTop.current = scroller.scrollTop;
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
      scroller.removeEventListener('scroll', track);
      observer?.disconnect();
    };
  }, []);

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

  return (
    <div className="message-stream" aria-live="polite">
      {entries.length === 0 &&
      !optimisticMessages?.length &&
      conversation.activeRunId === undefined ? (
        <p className="message-stream__empty">{emptyMessage}</p>
      ) : null}
      {entries.map((entry) => (
        <StreamEntry
          key={`${entry.kind}:${entry.item.id}`}
          entry={entry}
          approvalBusy={approvalBusy}
          approvalReadOnly={approvalReadOnly}
          onApproval={onApproval}
        />
      ))}
      {optimisticMessages?.map((message) => (
        <article className="message message--user" aria-label="You" key={`local:${message.id}`}>
          <span className="message__role">You</span>
          <p>{message.content}</p>
          <span className="message__delivery" role="status">
            {message.state === 'sending'
              ? 'Sending…'
              : message.state === 'accepted'
                ? 'Sent'
                : message.state === 'unconfirmed'
                  ? 'Awaiting confirmation'
                  : 'Not sent'}
          </span>
        </article>
      ))}
      {children}
      <div ref={endRef} />
    </div>
  );
}
