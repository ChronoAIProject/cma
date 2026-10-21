import type { ApprovalDecision } from './controller';
import type { ApprovalRequest } from './model';
import { Button } from '../ui/Button';
import './ApprovalCard.css';

/**
 * What each answer does, from how the runtime maps it: Allow selects the
 * provider's allow option, Deny its reject option, and Abort answers with
 * neither, cancelling the request.
 */
const CHOICES: readonly {
  readonly decision: ApprovalDecision;
  readonly label: string;
  readonly effect: string;
}[] = [
  {
    decision: 'deny',
    label: 'Deny',
    effect: 'Refuse this request. The Agent can continue another way.',
  },
  { decision: 'abort', label: 'Abort', effect: 'Cancel the request without an answer.' },
  { decision: 'allow', label: 'Allow', effect: 'Let the Agent go ahead with this request.' },
];

export function ApprovalCard({
  approval,
  busy,
  readOnly,
  onApproval,
}: {
  readonly approval: ApprovalRequest;
  readonly busy: boolean;
  readonly readOnly: boolean;
  readonly onApproval: (id: string, decision: ApprovalDecision) => void;
}) {
  const heading = `approval-${approval.id}`;
  return (
    <section className="approval" aria-labelledby={heading}>
      <div className="approval__request">
        <span className="approval__eyebrow">Approval</span>
        <h2 id={heading}>{approval.reason}</h2>
        {approval.message === undefined ? null : (
          <p className="approval__detail">{approval.message}</p>
        )}
      </div>
      {approval.decision === undefined && readOnly ? (
        <p className="approval__waiting">
          Waiting for an authorized participant to answer this approval.
        </p>
      ) : approval.decision === undefined ? (
        <div className="approval__actions">
          {CHOICES.map(({ decision, label, effect }) => (
            <div className="approval__choice" key={decision}>
              <Button
                size="sm"
                variant={decision === 'allow' ? 'primary' : 'outline'}
                disabled={busy}
                // The caption describes the answer; it never becomes part of its name.
                aria-describedby={`${heading}-${decision}`}
                onClick={() => onApproval(approval.id, decision)}
              >
                {label}
              </Button>
              <span className="approval__effect" id={`${heading}-${decision}`}>
                {effect}
              </span>
            </div>
          ))}
        </div>
      ) : (
        <span className="approval__decision">{approval.decision}</span>
      )}
    </section>
  );
}
