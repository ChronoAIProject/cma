import type { AgentLifecycleCreateReceipt, AgentLifecycleOperationView } from '../api/client';
import type { PreparationStage } from '../preparation/preparationPresentation';
import { StateDot } from '../ui/StateDot';
import {
  agentProvisioningSteps,
  orderedProvisioningEvents,
  provisioningDuration,
  provisioningStepDot,
  provisioningStepTiming,
  provisioningStepState,
} from './agentProvisioning';
import { useProvisioningClock } from './useProvisioningClock';
import './AgentLifecycleCard.css';

const slowPhaseAfterMs = 15_000;

const phaseActivity: Readonly<Record<string, string>> = {
  sandbox_ready: 'getting the cloud computer ready',
  workspace_runtime: 'preparing the workspace',
  key_scope_ready: 'setting up service access',
  agent_startup: 'starting the Agent',
  desktop_preparation: 'preparing the shared desktop',
};

/**
 * Agent setup as four reported milestones — the shared style contracts"Confirmed
 * Agent creation".
 *
 * There is deliberately no percentage. A bar that counted the four steps plus
 * success sat at 80% with every listed step Ready, which read as stalled; the
 * stepper says which milestone the server has confirmed and nothing more.
 */
export function AgentProvisioningProgress({
  receipt,
  operation,
  task,
}: {
  readonly receipt?: AgentLifecycleCreateReceipt;
  readonly operation?: AgentLifecycleOperationView;
  readonly task?: {
    readonly stage: PreparationStage;
    readonly label: string;
    readonly now: number;
    readonly phaseHistoryUnavailable: boolean;
  };
}) {
  const status =
    task?.stage === 'failed' ? 'failed' : (operation?.status ?? receipt?.status ?? 'queued');
  const events = orderedProvisioningEvents(operation);
  const active = ['queued', 'running', 'waiting_for_connection'].includes(status);
  const ownClock = useProvisioningClock(active && task === undefined);
  const now = task?.now ?? ownClock;
  const startedAt = task
    ? operation?.accepted_at
    : (operation?.accepted_at ?? events.find((event) => event.status === 'running')?.occurred_at);
  const timedEvents = events.filter((event) => Number.isFinite(Date.parse(event.occurred_at)));
  const completedAt = timedEvents
    .filter((event) =>
      status === 'failed'
        ? event.status === 'failed'
        : event.phase === 'agent_startup' && event.status === 'ready',
    )
    .at(-1)?.occurred_at;
  const lastKnownTime =
    task && !active && !completedAt ? timedEvents.at(-1)?.occurred_at : undefined;
  const elapsed = provisioningDuration(
    startedAt,
    active ? now : Date.parse(completedAt ?? lastKnownTime ?? ''),
  );
  const elapsedLabel = task
    ? lastKnownTime
      ? 'Setup elapsed at last update'
      : 'Setup elapsed'
    : 'Provisioning elapsed';
  const reportedSteps = events.some((event) => event.phase === 'desktop_preparation')
    ? [...agentProvisioningSteps, { phase: 'desktop_preparation', label: 'Desktop preparation' }]
    : agentProvisioningSteps;
  const steps = reportedSteps.map(({ phase, label }) => ({
    phase,
    label,
    state: task?.phaseHistoryUnavailable
      ? ('Not reported' as const)
      : provisioningStepState(phase, events, status, operation),
    ...provisioningStepTiming(phase, events, status, now),
  }));
  // Four Ready steps are not success: only the operation's own success with
  // the Agent it created may say the Agent is ready to open.
  const succeeded = status === 'succeeded' && operation?.agent_id !== undefined;
  const activeSteps = steps.filter(
    (step) => step.state === 'In progress' || step.state === 'Retrying',
  );
  const lastActive = events
    .filter((event) => activeSteps.some((step) => step.phase === event.phase))
    .at(-1);
  const current =
    activeSteps.find((step) => step.phase === operation?.phase) ??
    activeSteps.find((step) => step.phase === lastActive?.phase);
  const slow = current?.start !== undefined && now - Date.parse(current.start) >= slowPhaseAfterMs;
  const activity = current && phaseActivity[current.phase];
  let message = task?.label ?? 'Waiting for the next confirmed stage.';
  if (status === 'failed') {
    message = task?.label ?? operation?.error ?? 'Agent provisioning failed.';
  } else if (task?.stage === 'interrupted') {
    message = 'Checking setup updates';
  } else if (task?.stage === 'waiting' || status === 'waiting_for_connection') {
    message = task
      ? 'Waiting for connection. Connect the required service to continue.'
      : 'Connect the required services to continue setting up your Agent.';
  } else if (succeeded) {
    message = task?.label ?? 'Your Agent is ready to open.';
  } else if (task?.stage === 'stalled' || task?.stage === 'ready') {
    message = task.label;
  } else if (current) {
    message = `${current.label} · ${current.state}`;
    if (task && activity) {
      if (current.state === 'Retrying') message = `Still ${activity}. Setup is retrying.`;
      else if (slow) message = `Still ${activity}.`;
      else message = `${activity.charAt(0).toUpperCase()}${activity.slice(1)}.`;
    }
  }
  return (
    <section
      className="agent-lifecycle-provisioning"
      data-presentation={task ? 'task' : undefined}
      aria-label="Agent provisioning progress"
    >
      <h3>
        {task
          ? 'Setup progress'
          : status === 'failed'
            ? 'Agent provisioning needs attention'
            : succeeded
              ? 'Agent ready'
              : 'Preparing your Agent'}
      </h3>
      <p className="agent-lifecycle-provisioning__status" role="status">
        {task?.stage === 'preparing' && !task.phaseHistoryUnavailable && current === undefined ? (
          <StateDot state="ongoing" />
        ) : null}
        {message}
      </p>
      <ol className="agent-lifecycle-steps" aria-label="Agent setup steps">
        {steps.map(({ phase, label, state, duration }) => {
          const isCurrent =
            (task === undefined || task.stage === 'preparing') && phase === current?.phase;
          const dot = provisioningStepDot(state);
          const visibleDot = dot === 'ongoing' && !isCurrent ? 'neutral' : dot;
          return (
            <li key={phase} data-state={visibleDot} aria-current={isCurrent ? 'step' : undefined}>
              <StateDot state={visibleDot} placement="agent-lifecycle-steps__dot" />
              <strong>{label}</strong>
              <span className="agent-lifecycle-steps__state">
                {task && state === 'Needs attention' ? 'Failed' : state}
              </span>
              {duration === undefined ? null : <small>{duration} elapsed</small>}
            </li>
          );
        })}
      </ol>
      {elapsed === undefined ? null : (
        <p>
          {elapsedLabel}: {elapsed}
        </p>
      )}
    </section>
  );
}
