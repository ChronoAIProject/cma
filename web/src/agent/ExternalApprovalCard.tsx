import { CharacterAvatar } from '../character/CharacterAvatar';
import {
  approvalReviewUrl,
  type ExternalApprovalRequest,
  type ExternalApprovalReviewPolicy,
} from './externalApproval';
import './ApprovalCard.css';

export function ExternalApprovalCard({
  approval,
  policy,
  active,
  readOnly,
}: {
  readonly approval: ExternalApprovalRequest;
  readonly policy?: ExternalApprovalReviewPolicy;
  readonly active: boolean;
  readonly readOnly: boolean;
}) {
  const waiting = active && approval.decision === undefined;
  const href =
    waiting && !readOnly && approval.canReview
      ? approvalReviewUrl(approval.reviewUrl, policy)
      : undefined;
  const heading = `approval-${approval.id}`;
  return (
    <section className="approval" aria-labelledby={heading}>
      <div className="approval__head">
        {waiting ? (
          <CharacterAvatar
            pose="approval"
            tone="warning"
            size={40}
            className="approval__character"
          />
        ) : null}
        <div className="approval__request">
          <span className="approval__eyebrow">Approval</span>
          <h2 id={heading}>{waiting ? 'Waiting for your approval' : 'Approval request'}</h2>
          {waiting ? (
            <>
              <p className="approval__detail">This turn is waiting for approval in NyxID.</p>
              <p className="approval__detail">
                Approval deadline:{' '}
                <time dateTime={approval.deadlineAt}>
                  {new Date(approval.deadlineAt).toLocaleString()}
                </time>
                . The server confirms when the turn ends.
              </p>
            </>
          ) : (
            <p className="approval__decision">{approvalOutcome(approval.decision)}</p>
          )}
        </div>
      </div>
      {href ? (
        <a className="approval__review" href={href} target="_blank" rel="noopener noreferrer">
          Review and approve
        </a>
      ) : waiting ? (
        <p className="approval__waiting">
          {readOnly || !approval.canReview
            ? 'Waiting for an authorized participant to review this request in NyxID.'
            : 'The approval review link is unavailable. The turn is still waiting for approval.'}
        </p>
      ) : null}
    </section>
  );
}

function approvalOutcome(decision: string | undefined) {
  switch (decision) {
    case 'approved':
      return 'Approved';
    case 'denied':
      return 'Denied';
    case 'expired':
      return 'The approval request expired.';
    case 'disconnected':
      return 'The connection to this approval request ended.';
    case 'timed_out':
      return 'The approval wait timed out.';
    case 'cancelled':
      return 'The approval request was cancelled.';
    default:
      return 'This request is no longer active in the current turn.';
  }
}
