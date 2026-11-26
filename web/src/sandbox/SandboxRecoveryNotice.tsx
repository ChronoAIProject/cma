import type { SandboxDetail } from '../api/types';
import { StateDot } from '../ui/StateDot';
import { Button } from '../ui/Button';
import { IconCloseOutline16 } from '../ui/icons';
import { Presence } from '../ui/Presence';
import { lifecycleStageCopy, sandboxProgressCopy } from '../vocabulary/sandboxProgress';
import { sandboxHasParkedRuntimeUpdate } from './parkedRuntimeUpdate';
import { useRecoveryCompletion } from './useRecoveryCompletion';
import './SandboxRecoveryNotice.css';

function interruptionReason(sandbox: SandboxDetail): string | undefined {
  const interruption = sandbox.lifecycle?.interruption;
  if (interruption === undefined) return undefined;
  switch (interruption.cause) {
    case 'out_of_memory': {
      const gib = interruption.memoryLimitMib / 1024;
      return `The sandbox stopped because it ran out of memory. Configured memory: ${gib.toLocaleString(undefined, { maximumFractionDigits: 2 })} GiB.`;
    }
    case 'runtime_restarted':
      return 'The sandbox environment restarted unexpectedly.';
    case 'runtime_unavailable':
      return 'The sandbox stopped because of an internal error.';
  }
}

// Failure classes identify failed restoration or loss of an existing runtime.
// Ordinary pause -> retained reopen has no unexpected-interruption record.
export function sandboxRecoveryFailed(sandbox: SandboxDetail): boolean {
  return (
    sandbox.status === 'failed' &&
    ['restore_failed', 'runtime_lost'].includes(sandbox.failure?.class ?? '')
  );
}

export function SandboxRecoveryNotice({
  sandbox,
  stale = false,
  onDetails,
}: {
  readonly sandbox: SandboxDetail;
  readonly stale?: boolean;
  readonly onDetails?: () => void;
}) {
  const completion = useRecoveryCompletion(sandbox, stale);
  const operation = sandbox.lifecycle?.operation;
  const interruption = sandbox.lifecycle?.interruption;
  const recovering = sandbox.status === 'reviving' || operation?.purpose === 'revive';
  const parked = sandboxHasParkedRuntimeUpdate(sandbox);
  const failed = sandboxRecoveryFailed(sandbox);
  const cause = interruptionReason(sandbox);
  const stage = operation === undefined ? undefined : lifecycleStageCopy[operation.phase];
  const activeCause =
    interruption !== undefined &&
    operation !== undefined &&
    Date.parse(interruption.detectedAt) === Date.parse(operation.startedAt);
  const prolonged = sandbox.lifecycle?.result?.state === 'recoverable_pending';
  const title = parked
    ? 'Runtime update needs attention'
    : failed
      ? 'Sandbox recovery needs attention'
      : completion.completed
        ? 'Sandbox ready'
        : activeCause
          ? 'Recovering your sandbox'
          : 'Resuming — about 30 seconds';
  const visible = parked || recovering || failed || completion.completed;
  return (
    <>
      <Presence>
        {visible ? (
          <section
            className="sandbox-recovery"
            aria-label="Sandbox recovery"
            data-state={
              parked || failed ? 'failed' : completion.completed ? 'complete' : 'recovering'
            }
          >
            <div className="sandbox-recovery__icon" aria-hidden="true">
              <StateDot
                size={10}
                state={parked || failed ? 'error' : completion.completed ? 'done' : 'ongoing'}
              />
            </div>
            <div className="sandbox-recovery__content">
              <p className="sandbox-recovery__title" role="status">
                {title}
              </p>
              {((recovering && activeCause) || failed) && cause !== undefined ? (
                <p>
                  {failed ? 'Last recorded interruption: ' : ''}
                  {cause}
                </p>
              ) : null}
              {parked ? (
                <>
                  <p>{sandboxProgressCopy.parked}</p>
                  <p>Last confirmed stage: {stage?.label ?? sandboxProgressCopy.unknown}</p>
                </>
              ) : failed ? (
                <p role="alert">
                  {sandbox.failure?.remediation ??
                    'Recovery could not finish. Your conversation history remains available; review Sandbox details for the recorded failure.'}
                </p>
              ) : completion.completed ? (
                <p>Your sandbox is ready to continue.</p>
              ) : (
                <p key={stage?.label} className="sandbox-recovery__stage" aria-live="polite">
                  {stage?.label ?? 'Waiting for the next recorded recovery stage'}
                </p>
              )}
              {recovering && prolonged && !parked ? (
                <p>
                  Resume is taking longer. Your conversation is preserved; recovery is continuing.
                </p>
              ) : null}
              {stale ? (
                <p>Reconnecting to status updates. The last confirmed stage is shown.</p>
              ) : null}
              {onDetails === undefined || completion.completed ? null : (
                <Button size="sm" onClick={onDetails}>
                  Sandbox details
                </Button>
              )}
            </div>
            {completion.completed ? (
              <Button
                size="sm"
                icon={<IconCloseOutline16 />}
                aria-label="Dismiss recovery status"
                onClick={completion.dismiss}
              />
            ) : null}
          </section>
        ) : null}
      </Presence>
    </>
  );
}

/**
 * The last recorded interruption once the Sandbox has recovered from it.
 *
 * While recovery is still running or has failed, the notice above already
 * names the cause, so the history stays out of the way until then.
 */
export function SandboxInterruptionHistory({ sandbox }: { readonly sandbox: SandboxDetail }) {
  const interruption = sandbox.lifecycle?.interruption;
  const recovering =
    sandbox.status === 'reviving' || sandbox.lifecycle?.operation?.purpose === 'revive';
  if (interruption === undefined || recovering || sandboxRecoveryFailed(sandbox)) return null;
  return (
    <details className="sandbox-recovery-history">
      <summary>Last sandbox interruption</summary>
      <p>{interruptionReason(sandbox)}</p>
      <p>
        Detected{' '}
        <time dateTime={interruption.detectedAt}>
          {new Date(interruption.detectedAt).toLocaleString()}
        </time>
      </p>
    </details>
  );
}
