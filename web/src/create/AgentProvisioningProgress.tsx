import { useEffect, useState } from 'react';
import type { AgentLifecycleCreateReceipt, AgentLifecycleOperationView } from '../api/client';
import {
  agentProvisioningSteps,
  orderedProvisioningEvents,
  provisioningDuration,
  provisioningStepDuration,
  provisioningStepState,
} from './agentProvisioning';

export function AgentProvisioningProgress({
  receipt,
  operation,
}: {
  readonly receipt: AgentLifecycleCreateReceipt;
  readonly operation?: AgentLifecycleOperationView;
}) {
  const status = operation?.status ?? receipt.status ?? 'queued';
  const events = orderedProvisioningEvents(operation);
  const active = ['queued', 'running', 'waiting_for_connection'].includes(status);
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (!active) return;
    const tick = () => {
      if (!document.hidden) setNow(Date.now());
    };
    tick();
    const timer = setInterval(tick, 1000);
    document.addEventListener('visibilitychange', tick);
    return () => {
      clearInterval(timer);
      document.removeEventListener('visibilitychange', tick);
    };
  }, [active]);
  const startedAt = events.find((event) => event.status === 'running')?.occurred_at;
  const completedAt = events
    .filter((event) => event.phase === 'agent_startup' && event.status === 'ready')
    .at(-1)?.occurred_at;
  const elapsed = provisioningDuration(
    startedAt,
    active ? now : status === 'succeeded' ? Date.parse(completedAt ?? '') : NaN,
  );
  const steps = agentProvisioningSteps.map(({ phase, label }) => ({
    phase,
    label,
    state: provisioningStepState(phase, events, operation),
    duration: provisioningStepDuration(phase, events, status, now),
  }));
  const succeeded = status === 'succeeded' && operation?.agent_id !== undefined;
  const confirmed = succeeded ? 5 : steps.filter((step) => step.state === 'Ready').length;
  const percent = confirmed * 20;
  const current = steps.find((step) =>
    ['In progress', 'Waiting for connection', 'Needs attention'].includes(step.state),
  );
  return (
    <section
      className="agent-lifecycle-provisioning"
      aria-label="Agent provisioning progress"
      data-active={active ? '' : undefined}
    >
      <div
        className="agent-lifecycle-provisioning__circle"
        role="progressbar"
        aria-label="Confirmed Agent setup milestones"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent}
        aria-valuetext={`${confirmed} of 5 milestones confirmed`}
      >
        <svg viewBox="0 0 64 64" aria-hidden="true">
          <circle className="agent-lifecycle-provisioning__track" cx="32" cy="32" r="28" />
          <circle
            className="agent-lifecycle-provisioning__arc"
            cx="32"
            cy="32"
            r="28"
            pathLength="100"
            strokeDasharray={`${Math.max(percent, 8)} 100`}
          />
        </svg>
      </div>
      <div className="agent-lifecycle-provisioning__details">
        <h3>
          <strong>{percent}%</strong>
          {' · '}
          <span>
            {status === 'failed'
              ? 'Agent provisioning needs attention'
              : succeeded
                ? 'Agent ready'
                : 'Preparing your Agent'}
          </span>
        </h3>
        <p role="status">
          {status === 'failed'
            ? (operation?.error ?? 'Agent provisioning failed.')
            : status === 'waiting_for_connection'
              ? 'Connect the required services to continue setting up your Agent.'
              : succeeded
                ? 'Your Agent is ready to open.'
                : current === undefined
                  ? 'Waiting for the next confirmed stage.'
                  : `${current.label} · ${current.state}`}
        </p>
        {elapsed === undefined ? null : (
          <p className="agent-lifecycle-provisioning__elapsed">Provisioning elapsed: {elapsed}</p>
        )}
        <details>
          <summary>Setup details</summary>
          <ol>
            {steps.map(({ phase, label, state, duration }) => (
              <li
                key={phase}
                data-state={state}
                aria-current={state === 'In progress' ? 'step' : undefined}
              >
                <strong>{label}</strong>
                <span>{state}</span>
                {duration === undefined ? null : <small>{duration} elapsed</small>}
              </li>
            ))}
          </ol>
        </details>
      </div>
    </section>
  );
}
