import type { ApprovalDecision } from './controller';
import { IconRightUpOutline16, IconThinkOutline16, IconWarningOutline16 } from '../ui/icons';
import { MessageContent } from './MessageContent';
import { SignInCard } from './SignInCard';
import { ToolCallRow } from './ToolCallRows';
import { PlanCard } from './PlanCard';
import { ApprovalCard } from './ApprovalCard';
import { UserMessage } from './UserMessage';
import { TurnUsage } from './TurnUsage';
import type { AgentUsage } from '../api/types';
import { MessageAvatar, messageIdentity, type MessageUser } from './MessageIdentity';
import { MessageFooter } from './MessageFooter';
import type { MessageFeedbackControls } from './useMessageFeedback';
import type { Entry } from './timeline';

export function StreamEntry({
  entry,
  approvalBusy,
  approvalReadOnly = false,
  onApproval,
  activeRunId,
  showUsage = false,
  usage,
  user,
  unknownUserName,
  feedback,
}: {
  readonly entry: Entry;
  readonly approvalBusy: boolean;
  readonly approvalReadOnly?: boolean;
  readonly onApproval: (id: string, decision: ApprovalDecision) => void;
  /** Only the active turn can still be running a call. */
  readonly activeRunId?: string;
  readonly showUsage?: boolean;
  readonly usage?: AgentUsage['currentTurn'];
  readonly user?: MessageUser;
  readonly unknownUserName?: string;
  readonly feedback?: MessageFeedbackControls;
}) {
  if (entry.kind === 'message') {
    const message = entry.item;
    const identity = messageIdentity(message.role, user, unknownUserName);
    return (
      <article
        className={`message message--${message.role}`}
        aria-label={identity.name}
        data-streaming={message.status === 'streaming' ? '' : undefined}
        data-message-key={`message:${message.id}`}
      >
        <MessageAvatar identity={identity} />
        <div className="message__body">
          <span className="message__role">{identity.name}</span>
          {message.role === 'user' ? (
            <UserMessage content={message.content} />
          ) : (
            <MessageContent content={message.content} streaming={message.status === 'streaming'} />
          )}
          {showUsage ? (
            <div className="message__usage" aria-label="Turn token usage">
              <TurnUsage usage={usage} animateInitial={false} />
            </div>
          ) : null}
          <MessageFooter
            id={message.id}
            canVote={message.role === 'assistant'}
            at={message.at}
            content={message.content}
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
