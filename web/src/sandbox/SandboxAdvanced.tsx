import { Presence } from '../ui/Presence';
import { useEffect, useRef, useState } from 'react';
import type { CmaWebClient } from '../api/client';
import type { SandboxDetail, SandboxEvent, WorkspaceDetail } from '../api/types';
import { Modal } from '../ui/Modal';
import { Button } from '../ui/Button';
import { readSandboxHistory } from './eventHistory';
import { eventLabel } from '../vocabulary/status';
import { ProvisioningProgress } from './ProvisioningProgress';
import { sandboxProgress, workspaceProgress } from './progress';

export function SandboxAdvanced({
  client,
  sandbox,
  onClose,
}: {
  readonly client: CmaWebClient;
  readonly sandbox: SandboxDetail;
  readonly onClose: () => void;
}) {
  const [events, setEvents] = useState<readonly SandboxEvent[]>();
  const history = useRef<readonly SandboxEvent[]>([]);
  const [workspaces, setWorkspaces] = useState<readonly WorkspaceDetail[]>([]);
  const [error, setError] = useState<string>();
  const [retry, setRetry] = useState(0);
  useEffect(() => {
    const abort = new AbortController();
    void readSandboxHistory(client, sandbox.id, abort.signal, history.current)
      .then((value) => {
        if (!abort.signal.aborted) {
          history.current = value;
          setEvents(value);
          setError(undefined);
        }
      })
      .catch((cause: unknown) => {
        if (!abort.signal.aborted)
          setError(cause instanceof Error ? cause.message : 'Events unavailable.');
      });
    void Promise.all(
      sandbox.workspaces.map((workspace) =>
        client.readWorkspace(workspace.id, { signal: abort.signal }),
      ),
    )
      .then((value) => {
        if (!abort.signal.aborted)
          setWorkspaces(value.filter((workspace) => workspace.sandboxId === sandbox.id));
      })
      .catch(() => {});
    return () => abort.abort();
  }, [client, sandbox, retry]);
  const operation = sandbox.lifecycle?.operation;
  return (
    <Modal open title="Advanced sandbox details" onClose={onClose} placement="modal--wide">
      <div className="sandbox-advanced">
        <dl>
          <div>
            <dt>Sandbox ID</dt>
            <dd>
              <code>{sandbox.id}</code>
            </dd>
          </div>
          <div>
            <dt>Workspace limit</dt>
            <dd>{sandbox.limits.maxWorkspaces}</dd>
          </div>
          <div>
            <dt>Agents per workspace</dt>
            <dd>{sandbox.limits.maxAgentsPerWorkspace}</dd>
          </div>
          <div>
            <dt>Runtime version</dt>
            <dd>
              <code>{sandbox.runtimeVersion ?? 'Not reported'}</code>
            </dd>
          </div>
          <div>
            <dt>Persistent storage</dt>
            <dd>
              Agent Sandbox persists this Sandbox on its attached PV. CMA does not create, schedule,
              download or delete S3 snapshots.
            </dd>
          </div>
        </dl>
        {operation === undefined ? null : (
          <p>
            Current step: {operation.phase.replaceAll('_', ' ')}. Started{' '}
            <time dateTime={operation.startedAt}>
              {new Date(operation.startedAt).toLocaleString()}
            </time>
            .
          </p>
        )}
        {events !== undefined &&
        ['provisioning', 'installing', 'failed'].includes(sandbox.status) ? (
          <ProvisioningProgress
            title="Sandbox preparation"
            steps={sandboxProgress(sandbox, events)}
            failure={sandbox.failure}
          />
        ) : null}
        {workspaces.map((workspace) => (
          <section className="sandbox-page__band" key={workspace.id}>
            <h3>{workspace.name}</h3>
            <p>
              <code>{workspace.path}</code>
            </p>
            {events !== undefined && ['preparing', 'failed'].includes(workspace.status) ? (
              <ProvisioningProgress
                title={`${workspace.name} preparation`}
                steps={workspaceProgress(workspace, events)}
                failure={workspace.failure}
              />
            ) : null}
          </section>
        ))}
        <section className="sandbox-page__band">
          <h3>Events</h3>
          <Presence>
            {error !== undefined ? (
              <div role="alert">
                <p>{error}</p>
                <Button onClick={() => setRetry((value) => value + 1)}>Try again</Button>
              </div>
            ) : null}
          </Presence>
          {events === undefined ? (
            error === undefined ? (
              <p>Loading events</p>
            ) : null
          ) : events.length === 0 ? (
            <p>No events</p>
          ) : (
            <ol className="sandbox-page__events">
              {events.map((event) => (
                <li key={event.seq}>
                  <span>{eventLabel(event.kind.type)}</span>
                  <time dateTime={event.appendedAt}>
                    {new Date(event.appendedAt).toLocaleString()}
                  </time>
                </li>
              ))}
            </ol>
          )}
        </section>
      </div>
    </Modal>
  );
}
