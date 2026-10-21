import type { Launch, LaunchStep, LaunchStepStatus } from '@chronoai/cma-sdk';
import { LaunchSandboxProgress } from '../sandbox/LaunchSandboxProgress';
import { StateDot, type StateDotState } from '../ui/StateDot';
import { blockedStates, stepStates } from '../vocabulary/states';
import { progressDetail, provisioningNames, stepNames } from '../vocabulary/steps';

function combinedStatus(steps: readonly (LaunchStep | undefined)[]): LaunchStepStatus {
  for (const status of ['failed', 'blocked', 'retryable', 'running'] as const) {
    if (steps.some((step) => step?.status === status)) return status;
  }
  return steps.every((step) => step?.status === 'completed') ? 'completed' : 'pending';
}

export function LaunchProgress({ record }: { readonly record: Launch }) {
  const step = (key: LaunchStep['key']) => record.steps.find((item) => item.key === key);
  const sandbox = step('sandbox');
  const workspace = step('workspace');
  const agent = step('agent');
  // Launch records Agent readiness after runtime startup. Ordinary Agent creation
  // uses this same observation to publish workspace_runtime and key_scope_ready.
  // Connected accounts or a completed/skipped first message alone prove neither.
  const keyReady = [step('checking_requirements'), sandbox, workspace, agent].every(
    (item) => item?.status === 'completed',
  );
  const phases = [
    { key: 'sandbox', status: combinedStatus([sandbox]), steps: [sandbox] },
    { key: 'workspace', status: combinedStatus([workspace, agent]), steps: [workspace, agent] },
    { key: 'key_scope', status: keyReady ? 'completed' : 'pending', steps: [] },
  ] as const;
  const stateFor = (status: LaunchStepStatus) =>
    status === 'blocked' && record.blocked_reason?.type === 'sandbox_paused_reviving'
      ? 'running'
      : status;
  const paused = (status: LaunchStepStatus) =>
    status === 'blocked' && record.blocked_reason?.type === 'orchestration_paused';
  const labelFor = (status: LaunchStepStatus) =>
    paused(status) ? blockedStates.orchestration_paused : stepStates[stateFor(status)];
  // The same dot the in-app setup rows draw; CMA's own pause asks nothing of
  // the visitor, so it stays quiet rather than taking the Needs you warning.
  const dotFor = (status: LaunchStepStatus): StateDotState => {
    const state = stateFor(status);
    return state === 'completed'
      ? 'done'
      : state === 'running'
        ? 'ongoing'
        : state === 'failed'
          ? 'error'
          : state === 'blocked' && !paused(status)
            ? 'warning'
            : 'neutral';
  };
  const line = (name: string, status: LaunchStepStatus) => {
    const dot = dotFor(status);
    return (
      <div className="public-step__line">
        <span className="public-step__icon" aria-hidden="true">
          <StateDot state={dot} size={10} />
        </span>
        <h2>{name}</h2>
        <span
          className="public-step__state"
          data-tone={dot === 'warning' || dot === 'error' ? dot : undefined}
        >
          {labelFor(status)}
        </span>
      </div>
    );
  };
  const notice = (key: 'signing_in' | 'checking_requirements' | 'first_message') => {
    const observation = step(key);
    if (
      observation === undefined ||
      (key !== 'first_message' && observation.status === 'completed')
    )
      return null;
    const detail = progressDetail(observation.safe_detail);
    return (
      <section className="public-step" aria-label={stepNames[key]}>
        {line(stepNames[key], observation.status)}
        {detail === undefined ? null : <p className="public-step__detail">{detail}</p>}
      </section>
    );
  };
  const sandboxId =
    record.resolution.sandbox.state === 'pending' ? undefined : record.resolution.sandbox.id;
  const signInNotice = notice('signing_in');
  const requirementsNotice = notice('checking_requirements');
  const firstMessageNotice = notice('first_message');
  return (
    <>
      {signInNotice}
      {requirementsNotice}
      <ol className="public-timeline" aria-label="Agent provisioning progress">
        {phases.map((phase) => {
          const details = [
            ...new Set(phase.steps.map((item) => progressDetail(item?.safe_detail))),
          ];
          return (
            <li key={phase.key} className="public-step">
              {line(provisioningNames[phase.key], phase.status)}
              {details.map((detail) =>
                detail === undefined ? null : (
                  <p key={detail} className="public-step__detail">
                    {detail}
                  </p>
                ),
              )}
              {phase.key === 'sandbox' &&
              record.state === 'sandbox_pending' &&
              sandboxId !== undefined ? (
                <div className="public-step__detail">
                  <LaunchSandboxProgress key={sandboxId} sandboxId={sandboxId} />
                </div>
              ) : null}
            </li>
          );
        })}
      </ol>
      {firstMessageNotice}
    </>
  );
}
