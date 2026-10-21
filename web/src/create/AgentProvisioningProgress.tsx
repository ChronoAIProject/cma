import { useEffect, useState } from 'react';
import type { AgentLifecycleCreateReceipt, AgentLifecycleOperationView } from '../api/client';
import { StateDot } from '../ui/StateDot';
import {
  agentProvisioningSteps,
  orderedProvisioningEvents,
  provisioningDuration,
  provisioningStepDot,
  provisioningStepDuration,
  provisioningStepState,
} from './agentProvisioning';

/**
 * Agent setup as four reported milestones — `docs/Web_Style.md` §23, "Confirmed
 * Agent creation".
 *
 * There is deliberately no percentage. A bar that counted the four steps plus
 * success sat at 80% with every listed step Ready, which read as stalled; the
 * stepper says which milestone the server has confirmed and nothing more.
 */
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
  // Four Ready steps are not success: only the operation's own success with
  // the Agent it created may say the Agent is ready to open.
  const succeeded = status === 'succeeded' && operation?.agent_id !== undefined;
  const current = steps.find((step) =>
    ['In progress', 'Waiting for connection', 'Needs attention'].includes(step.state),
  );
  return (
    <section className="agent-lifecycle-provisioning" aria-label="Agent provisioning progress">
      <h3>
        {status === 'failed'
          ? 'Agent provisioning needs attention'
          : succeeded
            ? 'Agent ready'
            : 'Preparing your Agent'}
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
      <ol className="agent-lifecycle-steps" aria-label="Agent setup steps">
        {steps.map(({ phase, label, state, duration }) => (
          <li
            key={phase}
            data-state={provisioningStepDot(state)}
            aria-current={state === 'In progress' ? 'step' : undefined}
          >
            <StateDot state={provisioningStepDot(state)} placement="agent-lifecycle-steps__dot" />
            <strong>{label}</strong>
            <span className="agent-lifecycle-steps__state">{state}</span>
            {duration === undefined ? null : <small>{duration} elapsed</small>}
          </li>
        ))}
      </ol>
      {elapsed === undefined ? null : <p>Provisioning elapsed: {elapsed}</p>}
    </section>
  );
}
