import { useEffect, useRef } from 'react';
import type { ApprovalDecision } from './controller';
import type { ConversationState } from './conversation';
import { StreamEntry, type Entry } from './StreamEntry';

interface MessageStreamProps {
  readonly conversation: ConversationState;
  readonly approvalBusy?: boolean;
  readonly onApproval: (id: string, decision: ApprovalDecision) => void;
}

function count(value: number): string {
  return new Intl.NumberFormat().format(value);
}

export function MessageStream({
  conversation,
  approvalBusy = false,
  onApproval,
}: MessageStreamProps) {
  const endRef = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);

  useEffect(() => {
    const scroller = endRef.current?.closest('.agent-surface__conversation');
    if (!(scroller instanceof HTMLElement)) return;
    const track = () => {
      pinned.current = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight <= 48;
    };
    scroller.addEventListener('scroll', track, { passive: true });
    return () => scroller.removeEventListener('scroll', track);
  }, []);

  useEffect(() => {
    const end = endRef.current;
    if (pinned.current && end !== null && typeof end.scrollIntoView === 'function') {
      end.scrollIntoView({ block: 'end' });
    }
  }, [conversation]);

  const entries: Entry[] = [
    ...conversation.messages.map((item) => ({ kind: 'message' as const, item })),
    ...conversation.toolCalls.map((item) => ({ kind: 'tool' as const, item })),
    ...conversation.reasoning.map((item) => ({ kind: 'reasoning' as const, item })),
    ...conversation.approvals.map((item) => ({ kind: 'approval' as const, item })),
    ...conversation.recovery.map((item) => ({ kind: 'recovery' as const, item })),
  ];
  entries.sort((left, right) => (left.item.order ?? 0) - (right.item.order ?? 0));

  return (
    <div className="message-stream" aria-live="polite">
      {entries.length === 0 ? (
        <p className="message-stream__empty">Start the conversation.</p>
      ) : null}
      {entries.map((entry) => (
        <StreamEntry
          key={`${entry.kind}:${entry.item.id}`}
          entry={entry}
          approvalBusy={approvalBusy}
          onApproval={onApproval}
        />
      ))}
      {conversation.usage === undefined ? null : (
        <p className="message-stream__usage" aria-label="Agent usage">
          {count(conversation.usage.inputTokens)} input · {count(conversation.usage.outputTokens)}{' '}
          output
          {conversation.usage.contextWindow === undefined
            ? ` · ${count(conversation.usage.contextUsed)} in context`
            : ` · ${count(conversation.usage.contextUsed)} / ${count(conversation.usage.contextWindow)} context`}
        </p>
      )}
      <div ref={endRef} />
    </div>
  );
}
