import { useState } from 'react';
import type { ApprovalDecision } from './controller';
import type {
  ApprovalRequest,
  ConversationMessage,
  ReasoningBlock,
  RecoveryNotice,
  ToolCall,
} from './model';
import { Button } from '../ui/Button';
import {
  IconCodeOutline16,
  IconCopyOutline16,
  IconDislikeFill16,
  IconDislikeOutline16,
  IconLikeFill16,
  IconLikeOutline16,
  IconRightUpOutline16,
  IconThinkOutline16,
  IconWarningOutline16,
} from '../ui/icons';
import { MessageContent } from './MessageContent';
import { SignInCard } from './SignInCard';
import { ActivityBody } from './Thinking';
import { isPendingTool } from './toolStatus';

const SGT_FORMATTER = new Intl.DateTimeFormat('en-SG', {
  day: '2-digit',
  month: 'short',
  year: 'numeric',
  hour: '2-digit',
  minute: '2-digit',
  hour12: false,
  timeZone: 'Asia/Singapore',
  timeZoneName: 'short',
});

function messageTimestamp(value: string | undefined): string {
  if (value === undefined) return 'Time unavailable';
  const date = new Date(value);
  return Number.isFinite(date.getTime()) ? SGT_FORMATTER.format(date) : 'Time unavailable';
}

type Vote = 'up' | 'down' | undefined;

function MessageActions({ content }: { readonly content: string }) {
  const [vote, setVote] = useState<Vote>();
  const [copied, setCopied] = useState(false);

  const copy = async () => {
    try {
      if (navigator.clipboard?.writeText !== undefined) {
        await navigator.clipboard.writeText(content);
      } else {
        const field = document.createElement('textarea');
        field.value = content;
        field.setAttribute('readonly', '');
        field.style.position = 'fixed';
        field.style.opacity = '0';
        document.body.append(field);
        field.select();
        if (typeof document.execCommand === 'function' && !document.execCommand('copy')) {
          throw new Error('copy failed');
        }
        field.remove();
      }
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1600);
    } catch {
      setCopied(false);
    }
  };

  return (
    <div className="message__actions" aria-label="Message actions">
      <Button
        size="sm"
        variant="toolbar"
        icon={<IconCopyOutline16 />}
        aria-label={copied ? 'Message copied' : 'Copy message'}
        title={copied ? 'Message copied' : 'Copy message'}
        onClick={() => void copy()}
      />
      <Button
        size="sm"
        variant="toolbar"
        icon={vote === 'up' ? <IconLikeFill16 /> : <IconLikeOutline16 />}
        aria-label="Upvote message"
        title="Upvote message"
        aria-pressed={vote === 'up'}
        onClick={() => setVote((current) => (current === 'up' ? undefined : 'up'))}
      />
      <Button
        size="sm"
        variant="toolbar"
        icon={vote === 'down' ? <IconDislikeFill16 /> : <IconDislikeOutline16 />}
        aria-label="Downvote message"
        title="Downvote message"
        aria-pressed={vote === 'down'}
        onClick={() => setVote((current) => (current === 'down' ? undefined : 'down'))}
      />
    </div>
  );
}

export type Entry =
  | { kind: 'message'; item: ConversationMessage }
  | { kind: 'tool'; item: ToolCall }
  | { kind: 'reasoning'; item: ReasoningBlock }
  | { kind: 'approval'; item: ApprovalRequest }
  | { kind: 'recovery'; item: RecoveryNotice };

export function StreamEntry({
  entry,
  approvalBusy,
  approvalReadOnly = false,
  onApproval,
}: {
  readonly entry: Entry;
  readonly approvalBusy: boolean;
  readonly approvalReadOnly?: boolean;
  readonly onApproval: (id: string, decision: ApprovalDecision) => void;
}) {
  if (entry.kind === 'message') {
    const message = entry.item;
    return (
      <article
        className={`message message--${message.role}`}
        aria-label={message.role === 'user' ? 'You' : 'Agent'}
        data-streaming={message.status === 'streaming' ? '' : undefined}
      >
        <span className="message__role">{message.role === 'user' ? 'You' : 'Agent'}</span>
        {message.role === 'user' ? (
          <p>{message.content}</p>
        ) : (
          <MessageContent content={message.content} streaming={message.status === 'streaming'} />
        )}
        <div className="message__meta">
          <time
            className="message__timestamp"
            dateTime={message.at}
            title={message.at === undefined ? undefined : messageTimestamp(message.at)}
          >
            {messageTimestamp(message.at)}
          </time>
          <MessageActions content={message.content} />
        </div>
      </article>
    );
  }
  if (entry.kind === 'reasoning') {
    const reasoning = entry.item;
    return (
      <details className="agent-event">
        <summary>
          <IconThinkOutline16 />
          Reasoning<span>{reasoning.status === 'streaming' ? 'Thinking' : 'Complete'}</span>
        </summary>
        <p>{reasoning.content}</p>
      </details>
    );
  }
  if (entry.kind === 'tool') {
    const tool = entry.item;
    return (
      <article className="agent-event agent-event--tool" aria-label={`Tool call: ${tool.title}`}>
        <IconCodeOutline16 aria-hidden="true" />
        <div>
          <strong>{tool.title}</strong>
          <span>
            {[
              tool.kind,
              isPendingTool(tool)
                ? `Outcome not reported (last status: ${tool.status})`
                : tool.status,
            ]
              .filter(Boolean)
              .join(' · ')}
          </span>
          <span className="tool-call__duration">Duration not reported</span>
          <ActivityBody
            label="Details"
            body={tool.detail === undefined ? undefined : { text: tool.detail, truncated: false }}
          />
        </div>
      </article>
    );
  }
  if (entry.kind === 'recovery') {
    const notice = entry.item;
    if (notice.id.startsWith('sign-in:') || notice.id === 'live-sign-in')
      return <SignInCard notice={notice} />;
    const url =
      notice.url !== undefined && /^https?:\/\//i.test(notice.url) ? notice.url : undefined;
    return (
      <div className="message-stream__recovery" role="status">
        <IconWarningOutline16 aria-hidden="true" />
        <div>
          <strong>{notice.title}</strong>
          <p>{notice.detail}</p>
          {notice.code === undefined ? null : <code>{notice.code}</code>}
          {url === undefined ? null : (
            <a href={url} target="_blank" rel="noreferrer">
              Continue sign in <IconRightUpOutline16 />
            </a>
          )}
        </div>
      </div>
    );
  }
  const approval = entry.item;
  return (
    <section className="approval" aria-labelledby={`approval-${approval.id}`}>
      <div>
        <span className="approval__eyebrow">Approval</span>
        <h2 id={`approval-${approval.id}`}>{approval.reason}</h2>
        {approval.message === undefined ? null : <p>{approval.message}</p>}
      </div>
      {approval.decision === undefined && approvalReadOnly ? (
        <p>Waiting for an authorized participant to answer this approval.</p>
      ) : approval.decision === undefined ? (
        <div className="approval__actions">
          <Button size="sm" disabled={approvalBusy} onClick={() => onApproval(approval.id, 'deny')}>
            Deny
          </Button>
          <Button
            size="sm"
            disabled={approvalBusy}
            onClick={() => onApproval(approval.id, 'abort')}
          >
            Abort
          </Button>
          <Button
            size="sm"
            variant="primary"
            disabled={approvalBusy}
            onClick={() => onApproval(approval.id, 'allow')}
          >
            Allow
          </Button>
        </div>
      ) : (
        <span className="approval__decision">{approval.decision}</span>
      )}
    </section>
  );
}
