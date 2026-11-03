import { useId, useState } from 'react';
import {
  IconCheckOutline14,
  IconChevronDownOutline14,
  IconChevronRightOutline14,
  IconLoadingOutline16,
  IconQueueOutline14,
  IconQuestionOutline14,
} from '../ui/icons';
import type { StatusTone } from '../vocabulary/status';
import type { ConversationState, PlanSnapshot } from './model';
import { planAnchor, reportedPlanProgress } from './plans';
import '../ui/Badge.css';
import './PlanCard.css';

/**
 * The step words the rest of the product uses for progress. A status this
 * client does not know is shown as reported, muted, rather than mapped to the
 * nearest word, which would claim progress the Agent never stated.
 */
export function planStatus(status: string): { readonly word: string; readonly tone: StatusTone } {
  switch (status) {
    case 'completed':
      return { word: 'Done', tone: 'neutral' };
    case 'in_progress':
      return { word: 'In progress', tone: 'active' };
    case 'pending':
      return { word: 'Waiting', tone: 'muted' };
    default:
      return { word: status, tone: 'muted' };
  }
}

export function PlanCard({
  plan,
  active = true,
  outcome,
}: {
  readonly plan: PlanSnapshot;
  readonly active?: boolean;
  readonly outcome?: NonNullable<ConversationState['turnOutcomes']>[string];
}) {
  const [chosen, setChosen] = useState<boolean>();
  const open = chosen ?? active;
  const stepsId = useId();
  const { completed, currentIndex, multiple, known, summary } = reportedPlanProgress(plan);
  return (
    <section className="plan-card" aria-label="Plan" id={planAnchor(plan)} tabIndex={-1}>
      <span className="plan-card__eyebrow" aria-hidden="true">
        Plan
      </span>
      {active ? null : (
        <span className="plan-card__summary">
          {outcome === 'completed'
            ? known && completed === plan.entries.length && plan.entries.length > 0
              ? `Completed all ${completed} steps`
              : 'Response finished · Plan completion not fully reported'
            : outcome === 'failed'
              ? 'Response failed'
              : outcome === 'interrupted'
                ? 'Response cancelled'
                : 'Outcome unknown'}
        </span>
      )}
      <span className="plan-card__summary">{summary}</span>
      {known ? null : <span className="plan-card__current">Progress unknown</span>}
      {multiple ? <span className="plan-card__current">Multiple steps in progress</span> : null}
      {!active || currentIndex < 0 ? null : (
        <span className="plan-card__current">
          Current step {currentIndex + 1}: {plan.entries[currentIndex]!.title}
        </span>
      )}
      {known ? (
        <div
          className="plan-card__progress"
          role="progressbar"
          aria-label="Reported plan progress"
          aria-valuemin={0}
          aria-valuemax={plan.entries.length}
          aria-valuenow={completed}
          aria-valuetext={summary}
        >
          <span style={{ width: `${(completed / plan.entries.length) * 100}%` }} />
        </div>
      ) : null}
      <button
        type="button"
        className="plan-card__toggle"
        aria-expanded={open}
        aria-controls={open ? stepsId : undefined}
        onClick={() => setChosen(!open)}
      >
        {open ? <IconChevronDownOutline14 /> : <IconChevronRightOutline14 />}
        View plan and operations
      </button>
      {open ? (
        <ol className="plan-card__steps" id={stepsId}>
          {plan.entries.map((entry, index) => {
            const { word, tone } = planStatus(entry.status);
            return (
              <li className="plan-card__step" key={index}>
                <span className="plan-card__title">{entry.title}</span>
                <span className="badge" data-tone={tone}>
                  {entry.status === 'completed' ? (
                    <IconCheckOutline14 />
                  ) : entry.status === 'in_progress' ? (
                    <IconLoadingOutline16 />
                  ) : entry.status === 'pending' ? (
                    <IconQueueOutline14 />
                  ) : (
                    <IconQuestionOutline14 />
                  )}
                  <span className="badge__word">{word}</span>
                </span>
              </li>
            );
          })}
        </ol>
      ) : null}
    </section>
  );
}
