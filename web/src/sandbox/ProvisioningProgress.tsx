import { Presence } from '../ui/Presence';
import type { Failure } from '../api/types';
import type { ProvisioningStep } from './progress';
import './ProvisioningProgress.css';
import { StateDot } from '../ui/StateDot';

export function ProvisioningProgress({
  title,
  steps,
  failure,
}: {
  readonly title: string;
  readonly steps: readonly ProvisioningStep[];
  readonly failure?: Failure;
}) {
  return (
    <section className="provisioning-progress" aria-label={title}>
      <h2>{title}</h2>
      <ol aria-live="polite">
        {steps.map((step) => (
          <li key={step.label} data-state={step.status}>
            <span className="provisioning-progress__icon">
              <StateDot
                size={10}
                state={
                  step.status === 'Done'
                    ? 'done'
                    : step.status === 'In progress'
                      ? 'ongoing'
                      : step.status === "Didn't work"
                        ? 'error'
                        : 'neutral'
                }
              />
            </span>
            <span>{step.label}</span>
            <strong>{step.status}</strong>
            {step.detail === undefined ? null : <p>{step.detail}</p>}
          </li>
        ))}
      </ol>
      <Presence>
        {failure === undefined ? null : (
          <div className="provisioning-progress__failure" role="alert">
            <p>{failure.detail}</p>
            <p>
              {failure.remediation ?? 'Review the selected profile before creating a replacement.'}
            </p>
          </div>
        )}
      </Presence>
    </section>
  );
}
