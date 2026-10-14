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
  return (
    <section className="agent-lifecycle-provisioning" aria-label="Agent provisioning progress">
      <h3>
        {status === 'failed'
          ? 'Agent provisioning needs attention'
          : status === 'succeeded'
            ? 'Agent ready'
            : 'Preparing your Agent'}
      </h3>
      <p role="status">
        {status === 'failed'
          ? (operation?.error ?? 'Agent provisioning failed.')
          : status === 'waiting_for_connection'
            ? 'Connect the required services to continue setting up your Agent.'
            : status === 'succeeded'
              ? 'Your Agent is ready to open.'
              : 'Your Agent is being prepared. You can close this dialog and return to follow its progress.'}
      </p>
      {elapsed === undefined ? null : (
        <p className="agent-lifecycle-provisioning__elapsed">Provisioning elapsed: {elapsed}</p>
      )}
      <ol>
        {agentProvisioningSteps.map(({ phase, label }) => {
          const state = provisioningStepState(phase, events, operation);
          const duration = provisioningStepDuration(phase, events, status, now);
          return (
            <li
              key={phase}
              data-state={state}
              aria-current={state === 'In progress' ? 'step' : undefined}
            >
              <strong>{label}</strong>
              <span aria-live="polite">{state}</span>
              {duration === undefined ? null : <small>{duration} elapsed</small>}
            </li>
          );
        })}
      </ol>
    </section>
  );
}
