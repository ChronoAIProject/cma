import { memo } from 'react';
import type { ApprovalDecision } from './controller';
import { IconRightUpOutline16, IconThinkOutline16, IconWarningOutline16 } from '../ui/icons';
import { MessageContent } from './MessageContent';
import { SignInCard } from './SignInCard';
import { ToolCallRow } from './ToolCallRows';
import { PlanCard } from './PlanCard';
import { ApprovalCard } from './ApprovalCard';
import { UserMessage } from './UserMessage';
import { visibleUserMessage } from './externalEventMessage';
import { TurnUsage } from './TurnUsage';
import type { AgentUsage } from '../api/types';
import { MessageAvatar, messageIdentity, type MessageUser } from './MessageIdentity';
import { MessageFooter } from './MessageFooter';
import type { MessageFeedbackControls } from './useMessageFeedback';
import type { Entry } from './timeline';

function StreamEntryView({
  entry,
  approvalBusy,
  approvalReadOnly = false,
  onApproval,
  activeRunId,
  showUsage = false,
  usage,
  duration,
  user,
  unknownUserName,
  feedback,
  continued = false,
}: {
  readonly entry: Entry;
  readonly approvalBusy: boolean;
  readonly approvalReadOnly?: boolean;
  readonly onApproval: (id: string, decision: ApprovalDecision) => void;
  /** Only the active turn can still be running a call. */
  readonly activeRunId?: string;
  readonly showUsage?: boolean;
  readonly usage?: AgentUsage['currentTurn'];
  readonly duration?: string;
  readonly user?: MessageUser;
  readonly unknownUserName?: string;
  readonly feedback?: MessageFeedbackControls;
  /** The previous message has the same author, so this one does not repeat the identity. */
  readonly continued?: boolean;
}) {
  if (entry.kind === 'message') {
    const message = entry.item;
    const content = message.role === 'user' ? visibleUserMessage(message.content) : message.content;
    const identity = messageIdentity(message.role, user, unknownUserName);
    return (
      <article
        className={`message message--${message.role}`}
        aria-label={identity.name}
        data-streaming={message.status === 'streaming' ? '' : undefined}
        data-message-key={`message:${message.id}`}
      >
        <MessageAvatar identity={identity} continued={continued} />
        <div className="message__body">
          {continued ? null : <span className="message__role">{identity.name}</span>}
          {message.role === 'user' ? (
            <UserMessage content={content} />
          ) : (
            <MessageContent content={message.content} streaming={message.status === 'streaming'} />
          )}
          {/* A turn that reported nothing draws no line; absence is never shown as zero. */}
          {duration === undefined ? null : (
            <div className="message__duration" aria-label="Turn duration">
              {duration}
            </div>
          )}
          {showUsage && usage !== undefined ? (
            <div className="message__usage" aria-label="Turn token usage">
              <TurnUsage usage={usage} animateInitial={false} />
            </div>
          ) : null}
          <MessageFooter
            id={message.id}
            canVote={message.role === 'assistant'}
            at={message.at}
            content={content}
            committed={
              !message.liveOnly &&
              message.status !== 'streaming' &&
              /^event:[1-9]\d*$/.test(message.id)
            }
            feedback={feedback}
          />
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
  if (entry.kind === 'tool') return <ToolCallRow tool={entry.item} activeRunId={activeRunId} />;
  if (entry.kind === 'plan') return <PlanCard plan={entry.item} />;
  if (entry.kind === 'recovery') {
    const notice = entry.item;
    if (notice.clearsOn !== undefined)
      return (
        <p className="message__delivery" role="status">
          {notice.title}
        </p>
      );
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
  return (
    <ApprovalCard
      approval={entry.item}
      busy={approvalBusy}
      readOnly={approvalReadOnly}
      onApproval={onApproval}
    />
  );
}

export const StreamEntry = memo(StreamEntryView, (previous, next) => {
  if (previous.entry.kind !== next.entry.kind || previous.entry.item !== next.entry.item)
    return false;
  if (next.entry.kind === 'message')
    return (
      previous.showUsage === next.showUsage &&
      previous.usage === next.usage &&
      previous.duration === next.duration &&
      previous.user === next.user &&
      previous.unknownUserName === next.unknownUserName &&
      previous.feedback === next.feedback &&
      previous.continued === next.continued
    );
  if (next.entry.kind === 'tool') return previous.activeRunId === next.activeRunId;
  if (next.entry.kind === 'approval')
    return (
      previous.approvalBusy === next.approvalBusy &&
      previous.approvalReadOnly === next.approvalReadOnly &&
      previous.onApproval === next.onApproval
    );
  return true;
});
