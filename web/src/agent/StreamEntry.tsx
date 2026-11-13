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
import { MessageAvatar, messageIdentity, type MessageUser } from './MessageIdentity';
import { MessageFooter } from './MessageFooter';
import type { MessageFeedbackControls } from './useMessageFeedback';
import type { Entry } from './timeline';
import { AnswerDeliverables } from './AnswerDeliverables';
import { answerProse } from './answerDeliveries';
import type { ConversationState } from './model';

function StreamEntryView({
  entry,
  approvalBusy,
  approvalReadOnly = false,
  onApproval,
  activeRunId,
  planOutcome,
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
  readonly planOutcome?: NonNullable<ConversationState['turnOutcomes']>[string];
  readonly user?: MessageUser;
  readonly unknownUserName?: string;
  readonly feedback?: MessageFeedbackControls;
  /** The previous message has the same author, so this one does not repeat the identity. */
  readonly continued?: boolean;
}) {
  if (entry.kind === 'message') {
    const message = entry.item;
    const canDeliver =
      message.role === 'assistant' &&
      message.finalAnswer === true &&
      !message.liveOnly &&
      message.status !== 'streaming' &&
      /^event:[1-9]\d*$/.test(message.id);
    const delivery =
      message.role === 'assistant'
        ? answerProse(message.content, message.status === 'streaming')
        : { content: visibleUserMessage(message.content), references: 0 };
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
            <UserMessage content={delivery.content} />
          ) : (
            <MessageContent content={delivery.content} streaming={message.status === 'streaming'} />
          )}
          {canDeliver && delivery.references ? (
            <AnswerDeliverables answerId={message.id} references={delivery.references} />
          ) : null}
          <MessageFooter
            id={message.id}
            showVotes={message.role === 'assistant'}
            canVote={message.role === 'assistant' && message.finalAnswer === true}
            at={message.at}
            content={delivery.content}
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
  if (entry.kind === 'plan')
    return (
      <PlanCard
        plan={entry.item}
        active={entry.item.runId !== undefined && entry.item.runId === activeRunId}
        outcome={planOutcome}
      />
    );
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
      previous.user === next.user &&
      previous.unknownUserName === next.unknownUserName &&
      previous.feedback === next.feedback &&
      previous.continued === next.continued
    );
  if (next.entry.kind === 'tool') return previous.activeRunId === next.activeRunId;
  if (next.entry.kind === 'plan')
    return previous.activeRunId === next.activeRunId && previous.planOutcome === next.planOutcome;
  if (next.entry.kind === 'approval')
    return (
      previous.approvalBusy === next.approvalBusy &&
      previous.approvalReadOnly === next.approvalReadOnly &&
      previous.onApproval === next.onApproval
    );
  return true;
});
