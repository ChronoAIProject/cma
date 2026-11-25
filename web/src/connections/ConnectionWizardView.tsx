import { connectionCopy } from '../vocabulary/connect';
import { StateDot } from '../ui/StateDot';
import { Button } from '../ui/Button';
import { Modal } from '../ui/Modal';
import {
  subscriptionService,
  type ConnectionReport,
  type ConnectionTarget,
  type StartedConnection,
} from './client';
import { GitHubConnectionDetails, needsManualGitHubRepair } from './RepositoryConnectionRepair';

interface ViewProps {
  report: ConnectionReport | undefined;
  handoff: StartedConnection | undefined;
  running: boolean;
  busy: boolean;
  blocked: boolean;
  sameTabUrl: string | undefined;
  continueInSameTab: () => boolean;
  error: string | undefined;
  done: boolean;
  forceSubscription: boolean;
  description: string;
  presentation: 'inline' | 'dialog';
  dialogTitle: string;
  onCancel: (() => void) | undefined;
  onRetry: () => void;
  connect: () => void;
  cancel: () => void;
  authorize: () => Promise<void>;
  resumeRequired: boolean;
  repositoryRepair?: ConnectionTarget;
}

// The subscription row follows the ChatGPT sign-in the user sees.
function connectionName(row: { readonly service: string; readonly display_name: string }) {
  return row.service === subscriptionService ? 'Your ChatGPT subscription' : row.display_name;
}

export function ConnectionWizardView({
  report,
  handoff,
  running,
  busy,
  blocked,
  sameTabUrl,
  continueInSameTab,
  error,
  done,
  forceSubscription,
  description,
  presentation,
  dialogTitle,
  onCancel,
  onRetry,
  connect,
  cancel,
  authorize,
  resumeRequired,
  repositoryRepair: repositoryTarget,
}: ViewProps) {
  const connected = report?.services.filter((row) => row.state === 'satisfied').length ?? 0;
  const missing = report?.services.filter((row) => row.state !== 'satisfied') ?? [];
  const repositoryRepair = repositoryTarget && report && needsManualGitHubRepair(report);
  const content = (
    <section
      className="connection-wizard"
      aria-label="Required connections"
      aria-busy={busy || report === undefined}
    >
      <p>{description}</p>
      {report === undefined ? (
        <p role="status">Checking your connections…</p>
      ) : (
        <>
          <p className="connection-wizard__count" role="status">
            {connected} of {report.services.length} connections verified
          </p>
          {(['nyxid', 'codex'] as const).map((kind) => (
            <div className="connection-wizard__group" key={kind}>
              <h3>{kind === 'nyxid' ? 'NyxID services' : 'Model subscription'}</h3>
              {report.services
                .filter((row) => row.kind === kind)
                .map((row) => (
                  <div className="connection-wizard__row" key={row.service}>
                    <StateDot
                      size={10}
                      state={
                        row.state === 'satisfied'
                          ? 'done'
                          : row.service === handoff?.service_id
                            ? 'ongoing'
                            : 'warning'
                      }
                    />
                    <span>{connectionName(row)}</span>
                    <small>
                      {row.state === 'satisfied'
                        ? 'Connected'
                        : row.service === handoff?.service_id
                          ? 'Waiting for sign-in'
                          : 'Connection needed'}
                    </small>
                  </div>
                ))}
              {kind === 'nyxid' && !report.services.some((row) => row.kind === kind) ? (
                <p>No additional services required.</p>
              ) : null}
            </div>
          ))}
        </>
      )}
      {repositoryRepair ? (
        <>
          <p>
            Check your existing GitHub connection in NyxID. This task still needs verified
            repository access. If you have no suitable connection, add one in NyxID and then verify
            again.
          </p>
          <GitHubConnectionDetails owner={report.owner} target={repositoryTarget} />
        </>
      ) : handoff === undefined ? null : (
        <div className="connection-wizard__handoff">
          {handoff.user_code === undefined ? null : (
            <>
              <p>Sign in to your ChatGPT subscription using this one-time code:</p>
              <strong className="connection-wizard__code">{handoff.user_code}</strong>
              <p>
                Enter it on the OpenAI page. Your subscription stays connected to your CMA account.
              </p>
            </>
          )}
          <p role="status">
            {sameTabUrl !== undefined
              ? connectionCopy.blocked
              : blocked
                ? 'Open the sign-in window to continue. CMA will keep checking here.'
                : 'Finish sign-in in the other window. CMA is checking automatically…'}
          </p>
        </div>
      )}
      {resumeRequired ? (
        <p role="status">
          Sign-in was interrupted. Your original action is waiting for your confirmation.
        </p>
      ) : null}
      {error === undefined ? null : (
        <p role="alert" className="connection-wizard__error">
          {error}
        </p>
      )}
      <div className="connection-wizard__actions">
        {sameTabUrl === undefined || repositoryRepair ? null : (
          <a
            href={sameTabUrl}
            onClick={(event) => {
              if (!continueInSameTab()) event.preventDefault();
            }}
          >
            {connectionCopy.sameTab}
          </a>
        )}
        {repositoryRepair ? (
          <Button onClick={onRetry}>Verify again</Button>
        ) : done ? (
          <p>All connections verified.</p>
        ) : (
          <Button variant="primary" onClick={connect} disabled={busy || report === undefined}>
            {busy
              ? 'Preparing sign-in…'
              : resumeRequired && report?.ready && !forceSubscription
                ? 'Continue'
                : handoff !== undefined
                  ? 'Open sign-in window'
                  : forceSubscription
                    ? 'Sign in to your ChatGPT subscription again'
                    : missing.length === 1 && missing[0]?.service === subscriptionService
                      ? 'Sign in to your ChatGPT subscription'
                      : 'Connect all'}
          </Button>
        )}
        {error === undefined || repositoryRepair ? null : (
          <Button onClick={onRetry}>Verify again</Button>
        )}
        {!repositoryRepair &&
        report?.service_authorization_recovery === true &&
        !running &&
        onCancel !== undefined &&
        missing.some((row) => row.kind === 'nyxid') ? (
          <Button onClick={() => void authorize()} disabled={busy}>
            Authorize required services
          </Button>
        ) : null}
        {running || onCancel !== undefined ? <Button onClick={cancel}>Cancel</Button> : null}
      </div>
    </section>
  );
  if (presentation === 'inline') return content;
  // Check connected sessions without moving focus. Open recovery only when
  // the server reports work to do, or verification needs a retry.
  return (
    <Modal
      open={
        !done &&
        (resumeRequired ||
          error !== undefined ||
          (report !== undefined && (!report.ready || forceSubscription || handoff !== undefined)))
      }
      title={dialogTitle}
      onClose={cancel}
    >
      {content}
    </Modal>
  );
}
