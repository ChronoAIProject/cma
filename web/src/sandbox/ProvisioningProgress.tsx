import { Presence } from '../ui/Presence';
import type { Failure } from '../api/types';
import type { ProvisioningStep } from './progress';
import './ProvisioningProgress.css';
import { LandingCharacter } from '../landing/LandingCharacter';

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
            <LandingCharacter
              compact
              state={
                step.status === 'Done'
                  ? 'completed'
                  : step.status === 'In progress'
                    ? 'running'
                    : step.status === "Didn't work"
                      ? 'failed'
                      : 'pending'
              }
            />
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
