import type { SandboxDetail } from '../api/types';
import { Alert } from '../ui/Alert';
import { Button } from '../ui/Button';
import { Presence } from '../ui/Presence';
import { sandboxUnavailableNotice } from './lifecycle';
import { SandboxRecoveryNotice, sandboxRecoveryFailed } from './SandboxRecoveryNotice';

/**
 * Every message about the Sandbox, in one stack under its header —
 * `docs/Web_Style.md` §17.
 *
 * The page used to say the same fact in up to six separate paragraphs. Here
 * each fact appears once, in the tone of its severity: a refresh that failed
 * is a warning and keeps the content, a failure is an error with what to do,
 * and the state line is information with a way to its recorded progress.
 */
export function SandboxAlerts({
  sandbox,
  pollError,
  stale,
  onViewProgress,
}: {
  readonly sandbox: SandboxDetail;
  readonly pollError: boolean;
  readonly stale: boolean;
  /** Opens the Activity tab; omitted while it is already open. */
  readonly onViewProgress?: () => void;
}) {
  const failed = sandbox.status === 'failed';
  // A failure gets its own error alert, so the generic state line would only repeat it.
  const notice = failed ? undefined : sandboxUnavailableNotice(sandbox);
  return (
    <div className="alert-stack" data-sandbox-alerts="">
      <Presence>
        {pollError ? (
          <Alert tone="warning">Progress could not be refreshed. Reconnecting.</Alert>
        ) : null}
      </Presence>
      <SandboxRecoveryNotice sandbox={sandbox} stale={stale} />
      <Presence>
        {failed && !sandboxRecoveryFailed(sandbox) ? (
          <Alert tone="error" title="Your sandbox needs attention.">
            {sandbox.failure?.remediation ?? 'Open Technical details for the reported failure.'}
          </Alert>
        ) : null}
      </Presence>
      {notice === undefined ? null : (
        <Alert
          tone="info"
          actions={
            onViewProgress === undefined ? null : (
              <Button size="sm" onClick={onViewProgress}>
                View progress
              </Button>
            )
          }
        >
          {notice}
        </Alert>
      )}
    </div>
  );
}
