import { Badge } from '../ui/Badge';
import type { StatusTone } from '../vocabulary/status';
import type { PlanSnapshot } from './model';
import './PlanCard.css';

/**
 * The step words the rest of the product uses for progress. A status this
 * client does not know is shown as reported, muted, rather than mapped to the
 * nearest word, which would claim progress the Agent never stated.
 */
export function planStatus(status: string): { readonly word: string; readonly tone: StatusTone } {
  switch (status.toLowerCase()) {
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

export function PlanCard({ plan }: { readonly plan: PlanSnapshot }) {
  return (
    <section className="plan-card" aria-label="Plan">
      <span className="plan-card__eyebrow" aria-hidden="true">
        Plan
      </span>
      <ol className="plan-card__steps">
        {plan.entries.map((entry, index) => {
          const { word, tone } = planStatus(entry.status);
          return (
            // Steps carry no identity of their own, and every report replaces the whole list.
            <li className="plan-card__step" key={index}>
              <span className="plan-card__title">{entry.title}</span>
              <Badge tone={tone}>{word}</Badge>
            </li>
          );
        })}
      </ol>
    </section>
  );
}
