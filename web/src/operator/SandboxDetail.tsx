import { useEffect, useState } from 'react';
import type { AdminSandboxDetail, SandboxEvent } from '@chronoai/cma-sdk';
import { Button } from '../ui/Button';
import { IconRefreshOutline16 } from '../ui/icons';
import { Modal } from '../ui/Modal';
import type { OperatorClient } from './client';
import { operatorFailure, type OperatorFailure } from './failure';
import { FailureNotice, Timestamp, stateWord } from './records';
import { ReferenceLink } from '../profiles/ReferenceLink';

export function SandboxDetail({
  client,
  id,
  onClose,
}: {
  readonly client: OperatorClient;
  readonly id: string;
  readonly onClose: () => void;
}) {
  const [state, setState] = useState<{
    detail?: AdminSandboxDetail;
    events: readonly SandboxEvent[];
    error?: OperatorFailure;
    loading: boolean;
  }>({ events: [], loading: true });
  const [reload, setReload] = useState(0);
  const [after, setAfter] = useState(0);
  const [alwaysOnBusy, setAlwaysOnBusy] = useState(false);
  const [alwaysOnError, setAlwaysOnError] = useState<string>();
  useEffect(() => {
    const controller = new AbortController();
    Promise.all([
      client.admin.readSandbox(id, { signal: controller.signal }),
      client.admin.sandboxEvents(id, { after, limit: 50, signal: controller.signal }),
    ]).then(
      ([detail, events]) => {
        if (!controller.signal.aborted) setState({ detail, events, loading: false });
      },
      (error: unknown) => {
        if (!controller.signal.aborted)
          setState((previous) => ({ ...previous, error: operatorFailure(error), loading: false }));
      },
    );
    return () => controller.abort();
  }, [client, id, after, reload]);
  const detail = state.detail;
  return (
    <Modal open title="Sandbox details" description={detail?.sandbox.name} onClose={onClose}>
      <div className="operator-detail">
        <Button
          size="sm"
          disabled={state.loading}
          aria-label="Refresh details"
          title="Refresh details"
          icon={<IconRefreshOutline16 />}
          onClick={() => {
            setState((previous) => ({ ...previous, loading: true, error: undefined }));
            setReload((value) => value + 1);
          }}
        />
        <FailureNotice failure={state.error} />
        {state.loading ? (
          <div className="operator-skeleton" role="status" aria-label="Loading details">
            <span />
            <span />
            <span />
          </div>
        ) : null}
        {detail === undefined ? null : (
          <>
            <section>
              <h3>Sandbox</h3>
              <dl>
                <dt>Status</dt>
                <dd className="operator-state">{stateWord(detail.sandbox.status)}</dd>
                <dt>Owner</dt>
                <dd>{detail.owner_name?.trim() || 'Name unavailable'}</dd>
                <dt>Identifier</dt>
                <dd>
                  <code>{detail.sandbox.id}</code>
                </dd>
                <dt>Runtime</dt>
                <dd>{detail.sandbox.runtime?.runtime_version ?? 'not bound'}</dd>
                <dt>Profile</dt>
                <dd>
                  <ReferenceLink kind="sandbox" id={detail.sandbox.profile.id} /> revision{' '}
                  {detail.sandbox.profile.revision}
                </dd>
                <dt>Always on</dt>
                <dd>
                  <label className="operator-switch">
                    <input
                      type="checkbox"
                      checked={detail.sandbox.profile.always_on === true}
                      disabled={alwaysOnBusy || state.loading}
                      onChange={(event) => {
                        const value = event.target.checked;
                        setAlwaysOnBusy(true);
                        setAlwaysOnError(undefined);
                        void client.admin
                          .setSandboxAlwaysOn(
                            id,
                            { always_on: value, expected_revision: detail.sandbox.revision },
                            {},
                          )
                          .then(() => {
                            setState((previous) =>
                              previous.detail === undefined
                                ? previous
                                : {
                                    ...previous,
                                    detail: {
                                      ...previous.detail,
                                      sandbox: {
                                        ...previous.detail.sandbox,
                                        profile: {
                                          ...previous.detail.sandbox.profile,
                                          always_on: value,
                                        },
                                        revision: previous.detail.sandbox.revision + 1,
                                      },
                                    },
                                  },
                            );
                          })
                          .catch((error: unknown) => {
                            setAlwaysOnError(
                              error instanceof Error
                                ? error.message
                                : 'Could not save this control.',
                            );
                          })
                          .finally(() => setAlwaysOnBusy(false));
                      }}
                    />
                    Keep this Sandbox running when automatic pause is enabled
                  </label>
                  {alwaysOnError === undefined ? null : <p role="alert">{alwaysOnError}</p>}
                </dd>
                <dt>Created</dt>
                <dd>
                  <Timestamp value={detail.sandbox.created_at} />
                </dd>
              </dl>
              {detail.sandbox.failure === undefined ? null : (
                <p className="operator-failure">
                  {detail.sandbox.failure.detail} <code>{detail.sandbox.failure.class}</code>
                </p>
              )}
            </section>
            <section>
              <h3>Workspaces ({detail.workspaces.length})</h3>
              {detail.workspaces.length === 0 ? (
                <p>No Workspaces.</p>
              ) : (
                <ul>
                  {detail.workspaces.map((workspace) => (
                    <li key={workspace.id}>
                      <p>
                        {workspace.name}: {stateWord(workspace.status)}
                      </p>
                      <p>
                        Profile: <ReferenceLink kind="workspace" id={workspace.profile_id} />{' '}
                        revision {workspace.profile_revision}
                      </p>
                      <h4>
                        Agents (
                        {
                          detail.agents.filter((agent) => agent.workspace_id === workspace.id)
                            .length
                        }
                        )
                      </h4>
                      {detail.agents.some((agent) => agent.workspace_id === workspace.id) ? (
                        <ul>
                          {detail.agents
                            .filter((agent) => agent.workspace_id === workspace.id)
                            .map((agent) => (
                              <li key={agent.id}>
                                <p>
                                  {agent.title?.trim() || 'Untitled agent'}:{' '}
                                  {stateWord(agent.status)}
                                </p>
                                <p>
                                  Profile: <ReferenceLink kind="agent" id={agent.profile_id} />{' '}
                                  revision {agent.profile_revision}
                                </p>
                                <p>
                                  Last activity <Timestamp value={agent.updated_at} />
                                </p>
                              </li>
                            ))}
                        </ul>
                      ) : (
                        <p>No Agents.</p>
                      )}
                    </li>
                  ))}
                </ul>
              )}
            </section>
            <section>
              <h3>Event timeline</h3>
              {state.events.length === 0 ? (
                <p>No events in this page.</p>
              ) : (
                <ol className="operator-timeline">
                  {state.events.map((event) => (
                    <li key={event.seq}>
                      <Timestamp value={event.appended_at} />
                      <p>{stateWord(event.kind.type)}</p>
                      <EventDetail event={event} />
                      <code>Sequence {event.seq}</code>
                    </li>
                  ))}
                </ol>
              )}
              <div className="operator-actions">
                <Button
                  size="sm"
                  disabled={after === 0 || state.loading}
                  onClick={() => {
                    setState((previous) => ({ ...previous, loading: true, error: undefined }));
                    setAfter(0);
                  }}
                >
                  First events
                </Button>
                <Button
                  size="sm"
                  disabled={
                    state.loading ||
                    state.events.length < 50 ||
                    (state.events.at(-1)?.seq ?? 0) <= after
                  }
                  onClick={() => {
                    setState((previous) => ({ ...previous, loading: true, error: undefined }));
                    setAfter(state.events.at(-1)?.seq ?? 0);
                  }}
                >
                  Next events
                </Button>
              </div>
            </section>
          </>
        )}
      </div>
    </Modal>
  );
}

function EventDetail({ event }: { readonly event: SandboxEvent }) {
  const kind = event.kind;
  if ('failure' in kind)
    return (
      <p>
        {kind.failure.detail} <code>{kind.failure.class}</code>
      </p>
    );
  if ('from' in kind)
    return (
      <p>
        {stateWord(kind.from)} to {stateWord(kind.to)}
      </p>
    );
  if ('workspace_id' in kind)
    return (
      <p>
        Workspace <code>{kind.workspace_id}</code>
      </p>
    );
  if ('agent_id' in kind)
    return (
      <p>
        Agent <code>{kind.agent_id}</code>
      </p>
    );
  if ('step' in kind) return <p>{stateWord(kind.step.kind)}</p>;
  if ('runtime_version' in kind) return <p>{kind.runtime_version}</p>;
  return null;
}
