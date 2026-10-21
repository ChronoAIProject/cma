import type { ApprovalDecision } from './controller';
import { Button } from '../ui/Button';
import {
  IconCodeOutline16,
  IconRightUpOutline16,
  IconThinkOutline16,
  IconWarningOutline16,
} from '../ui/icons';
import { MessageContent } from './MessageContent';
import { SignInCard } from './SignInCard';
import { ActivityBody } from './Thinking';
import { isPendingTool } from './toolStatus';
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
