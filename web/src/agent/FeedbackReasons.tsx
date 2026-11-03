import { useId } from 'react';
import { Button } from '../ui/Button';
import { feedbackReasons, type FeedbackReason } from '../api/feedbackTypes';
import type { MessageFeedbackControls } from './useMessageFeedback';
import './FeedbackReasons.css';

const labels: Record<FeedbackReason, string> = {
  inaccurate: 'Inaccurate content',
  unanswered: 'Did not answer the question',
  unusable_steps: 'Steps do not work',
  incomplete: 'Incomplete answer',
  unclear: 'Unclear wording',
  other: 'Other',
};

export function FeedbackReasons({
  id,
  feedback,
}: {
  readonly id: string;
  readonly feedback: MessageFeedbackControls;
}) {
  const fieldId = useId();
  const controls = feedback.reasons;
  if (!controls) return null;
  if (controls.denied) return <span role="status">Feedback is unavailable for this account.</span>;
  const row = controls.rows[id];
  const vote = feedback.rows[id];
  if (!row?.open)
    return vote?.vote === 'down' ? (
      <Button size="sm" onClick={() => controls.open(id)}>
        Edit reasons
      </Button>
    ) : null;
  const count = Array.from(row.draft.comment).length;
  const overLimit = count > 500;
  const locked = row.busy === 'save' || Boolean(row.draft.pending);
  return (
    <section className="feedback-reasons" aria-label="Reasons for improvement">
      <fieldset disabled={locked}>
        <legend>Reasons for improvement</legend>
        {feedbackReasons.map((reason) => (
          <label key={reason}>
            <input
              type="checkbox"
              checked={row.draft.reasons.includes(reason)}
              onChange={(event) =>
                controls.edit(id, {
                  reasons: event.target.checked
                    ? [...row.draft.reasons, reason]
                    : row.draft.reasons.filter((value) => value !== reason),
                  comment: row.draft.comment,
                })
              }
            />
            {labels[reason]}
          </label>
        ))}
      </fieldset>
      <label htmlFor={fieldId}>Additional details (optional)</label>
      <textarea
        id={fieldId}
        value={row.draft.comment}
        disabled={locked}
        aria-invalid={overLimit || undefined}
        aria-describedby={`${fieldId}-count`}
        onChange={(event) =>
          controls.edit(id, { reasons: row.draft.reasons, comment: event.target.value })
        }
      />
      <span id={`${fieldId}-count`}>
        {count} / 500{overLimit ? ' — Use no more than 500 characters.' : ''}
      </span>
      <div className="feedback-reasons__actions">
        <Button
          size="sm"
          disabled={
            !feedback.ready ||
            vote?.vote !== 'down' ||
            vote.pending ||
            !row.details ||
            row.details.vote !== 'down' ||
            Boolean(row.busy) ||
            row.conflict ||
            overLimit
          }
          onClick={() => controls.save(id)}
        >
          {row.draft.pending ? 'Retry reasons' : 'Save reasons'}
        </Button>
        <Button size="sm" onClick={() => controls.close(id)}>
          Close
        </Button>
        {row.error || (row.details && row.details.vote !== 'down' && !vote?.pending) ? (
          <Button
            size="sm"
            disabled={vote?.pending}
            onClick={() => {
              feedback.reload();
              controls.open(id);
            }}
          >
            Reload feedback
          </Button>
        ) : null}
      </div>
      {row.busy ? (
        <span role="status">{row.busy === 'save' ? 'Saving reasons…' : 'Loading reasons…'}</span>
      ) : null}
      {row.saved ? <span role="status">Reasons saved</span> : null}
      {row.error ? <span role="status">{row.error}</span> : null}
      {row.details && row.details.vote !== 'down' && !vote?.pending ? (
        <span role="status">Confirm a needs improvement vote before saving reasons.</span>
      ) : null}
    </section>
  );
}
