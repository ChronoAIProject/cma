import { useLayoutEffect, useRef } from 'react';
import type { ApprovalDecision } from './controller';
import type { ConversationState } from './conversation';
import { StreamEntry, type Entry } from './StreamEntry';
import { Thinking } from './Thinking';
import { BrandLoading } from '../landing/LandingCharacter';

interface MessageStreamProps {
  readonly conversation: ConversationState;
  readonly approvalBusy?: boolean;
  readonly onApproval: (id: string, decision: ApprovalDecision) => void;
}

export function MessageStream({
  conversation,
  approvalBusy = false,
  onApproval,
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
  }, [conversation]);

  const entries: Entry[] = [
    ...conversation.messages.map((item) => ({ kind: 'message' as const, item })),
    ...conversation.toolCalls.map((item) => ({ kind: 'tool' as const, item })),
    ...conversation.reasoning.map((item) => ({ kind: 'reasoning' as const, item })),
    ...conversation.approvals.map((item) => ({ kind: 'approval' as const, item })),
    ...conversation.recovery.map((item) => ({ kind: 'recovery' as const, item })),
  ];
  entries.sort((left, right) => (left.item.order ?? 0) - (right.item.order ?? 0));
  const thinking = new Map<string, Extract<Entry, { kind: 'tool' | 'reasoning' }>[]>();
  let turn = 'initial';
  const groups = new Map<Entry, string>();
  for (const entry of entries) {
    if (entry.kind === 'message' && entry.item.role === 'user')
      turn = entry.item.runId ?? entry.item.id;
    if (entry.kind !== 'tool' && entry.kind !== 'reasoning') continue;
    const key = entry.item.runId ?? turn;
    const group = thinking.get(key) ?? [];
    group.push(entry);
    thinking.set(key, group);
    groups.set(entry, key);
  }
  const rendered = new Set<string>();

  return (
    <div className="message-stream" aria-live="polite">
      {entries.length === 0 ? (
        <p className="message-stream__empty">Start the conversation.</p>
      ) : null}
      {entries.map((entry) => {
        const key = groups.get(entry);
        if (key !== undefined) {
          if (rendered.has(key)) return null;
          rendered.add(key);
          const group = thinking.get(key) ?? [];
          return (
            <Thinking
              key={`thinking:${key}`}
              live={conversation.activeRunId === key}
              text={group
                .flatMap((item) => (item.kind === 'reasoning' ? [item.item.content] : []))
                .join('\n')}
              activities={group.flatMap((item) =>
                item.kind === 'tool'
                  ? [
                      {
                        id: item.item.id,
                        kind: item.item.kind ?? 'Tool',
                        target: item.item.title,
                        status: item.item.status,
                        detail:
                          item.item.detail === undefined
                            ? undefined
                            : { text: item.item.detail, truncated: false },
                      },
                    ]
                  : [],
              )}
            />
          );
        }
        return (
          <StreamEntry
            key={`${entry.kind}:${entry.item.id}`}
            entry={entry}
            approvalBusy={approvalBusy}
            onApproval={onApproval}
          />
        );
      })}
      {conversation.activeRunId !== undefined &&
      !entries.some(
        (entry) =>
          'runId' in entry.item &&
          entry.item.runId === conversation.activeRunId &&
          entry.kind !== 'message',
      ) &&
      !conversation.messages.some(
        (message) => message.runId === conversation.activeRunId && message.role === 'assistant',
      ) ? (
        <BrandLoading label="Working" />
      ) : null}
      <div ref={endRef} />
    </div>
  );
}
