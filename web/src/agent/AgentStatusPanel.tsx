import type { AgentDetail, SandboxEvent } from '../api/types';
import { CharacterAvatar } from '../character/CharacterAvatar';
import { ProvisioningProgress } from '../sandbox/ProvisioningProgress';
import { agentProgress } from '../sandbox/progress';
import { StateDot } from '../ui/StateDot';
import type { AgentController } from './controller';
import './AgentStatusPanel.css';

export function AgentStatusPanel({
  detail,
  events = [],
  observation,
  running,
  historyIncomplete,
}: {
  readonly detail: AgentDetail;
  readonly events?: readonly SandboxEvent[];
  readonly observation: AgentController['progressObservation'];
  readonly running: boolean;
  readonly historyIncomplete: boolean;
}) {
  const steps = agentProgress(detail, events);
  const preparing = detail.status === 'starting' || steps.some((step) => step.status !== 'Done');
  const failed = detail.failure !== undefined || (preparing && detail.status === 'failed');
  const working = running && ['ready', 'working'].includes(detail.status);
  if (!preparing && !failed && !working) return null;
  const observed = observation?.sandbox.id === detail.sandbox.id ? observation : undefined;
  const uncertain = observed === undefined || observed.stale || (running && historyIncomplete);
  const title = failed
    ? 'Agent needs attention'
    : uncertain
      ? preparing
        ? 'We can’t confirm setup progress'
        : 'We can’t confirm the latest activity'
      : preparing
        ? 'Getting its cloud computer ready'
        : 'Working on it';
  const confirmed = preparing ? 'Preparing' : 'Working';
  return (
    <section
      className="agent-status-panel"
      aria-label={preparing || failed ? 'Agent preparation' : 'Agent activity'}
      data-layout={preparing && !uncertain && !failed ? 'preparing' : 'compact'}
    >
      <CharacterAvatar
        pose={failed ? 'failed' : uncertain ? 'thinking' : preparing ? 'preparing' : 'working'}
        size={preparing && !uncertain && !failed ? 96 : 56}
      />
      <div className="agent-status-panel__body">
        <div aria-live="polite">
          <h2>{title}</h2>
          <p className="agent-status-panel__state">
            <StateDot state={failed ? 'error' : uncertain ? 'neutral' : 'ongoing'} />
            {failed
              ? 'Setup needs attention'
              : uncertain
                ? `Last confirmed: ${confirmed}`
                : confirmed}
          </p>
          {uncertain ? (
            <p>The latest state is unavailable. This does not confirm that the task failed.</p>
          ) : null}
          {preparing && !failed && !uncertain ? (
            <p>Setup progress will appear here as it is confirmed.</p>
          ) : null}
          {observed?.observedAt && Number.isFinite(Date.parse(observed.observedAt)) && preparing ? (
            <p className="agent-status-panel__time">
              Environment last checked:{' '}
              <time dateTime={observed.observedAt}>
                {new Date(observed.observedAt).toLocaleTimeString()}
              </time>
            </p>
          ) : null}
        </div>
        {preparing || failed ? (
          <details className="agent-status-panel__details">
            <summary>Setup details</summary>
            <ProvisioningProgress
              title="Recorded setup progress"
              steps={steps}
              failure={detail.failure}
            />
          </details>
        ) : null}
      </div>
    </section>
  );
}
