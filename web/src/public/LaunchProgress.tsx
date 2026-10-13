import type { Launch, LaunchStep, LaunchStepStatus } from '@chronoai/cma-sdk';
import { LandingCharacter } from '../landing/LandingCharacter';
import { LaunchSandboxProgress } from '../sandbox/LaunchSandboxProgress';
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
  const labelFor = (status: LaunchStepStatus) =>
    status === 'blocked' && record.blocked_reason?.type === 'orchestration_paused'
      ? blockedStates.orchestration_paused
      : stepStates[stateFor(status)];
  const notice = (key: 'signing_in' | 'checking_requirements' | 'first_message') => {
    const observation = step(key);
    if (
      observation === undefined ||
      (key !== 'first_message' && observation.status === 'completed')
    )
      return null;
    const detail = progressDetail(observation.safe_detail);
    return (
      <section className="public-notice" aria-label={stepNames[key]}>
        <div className="public-step__line">
          <h2>{stepNames[key]}</h2>
          <span>{labelFor(observation.status)}</span>
        </div>
        {detail === undefined ? null : <p>{detail}</p>}
      </section>
    );
  };
  const sandboxId =
    record.resolution.sandbox.state === 'pending' ? undefined : record.resolution.sandbox.id;
  return (
    <>
      {notice('signing_in')}
      {notice('checking_requirements')}
      <ol className="public-timeline" aria-label="Agent provisioning progress">
        {phases.map((phase) => {
          const state = stateFor(phase.status);
          const details = [
            ...new Set(phase.steps.map((item) => progressDetail(item?.safe_detail))),
          ];
          return (
            <li
              key={phase.key}
              className={`public-step${state === 'failed' ? ' public-step--failed' : ''}`}
            >
              <span className="public-step__icon" aria-hidden="true">
                <LandingCharacter
                  compact
                  state={
                    state === 'completed' || state === 'failed' || state === 'running'
                      ? state
                      : 'pending'
                  }
                />
              </span>
              <div>
                <div className="public-step__line">
                  <h2>{provisioningNames[phase.key]}</h2>
                  <span>{labelFor(phase.status)}</span>
                </div>
                {details.map((detail) =>
                  detail === undefined ? null : <p key={detail}>{detail}</p>,
                )}
                {phase.key === 'sandbox' &&
                record.state === 'sandbox_pending' &&
                sandboxId !== undefined ? (
                  <LaunchSandboxProgress key={sandboxId} sandboxId={sandboxId} />
                ) : null}
              </div>
            </li>
          );
        })}
      </ol>
      {notice('first_message')}
    </>
  );
}
