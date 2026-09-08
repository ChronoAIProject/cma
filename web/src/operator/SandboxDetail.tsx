import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import type { AdminSandboxDetail, SandboxEvent } from '@chronoai/cma-sdk';
import { Button } from '../ui/Button';
import { IconCloseOutline16, IconRefreshOutline16 } from '../ui/icons';
import { usePlateFocus } from '../ui/plate';
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
  const panel = useRef<HTMLDivElement>(null);
  usePlateFocus(panel, onClose);
  const [state, setState] = useState<{
    detail?: AdminSandboxDetail;
    events: readonly SandboxEvent[];
    error?: OperatorFailure;
    loading: boolean;
  }>({ events: [], loading: true });
  const [reload, setReload] = useState(0);
  const [after, setAfter] = useState(0);
  useEffect(() => {
    const previous = document.body.style.overflow;
    document.body.style.overflow = 'hidden';
    return () => {
      document.body.style.overflow = previous;
    };
  }, []);
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
  return createPortal(
    <div className="operator-drawer-mask" role="presentation" onClick={onClose}>
      <div
        className="operator-detail operator-drawer"
        role="dialog"
        aria-modal="true"
        aria-label="Sandbox details"
        ref={panel}
        onClick={(event) => event.stopPropagation()}
      >
        <header>
          <h2>{detail?.sandbox.name ?? 'Sandbox details'}</h2>
          <Button
            size="sm"
            aria-label="Close details"
            title="Close details"
            icon={<IconCloseOutline16 />}
            onClick={onClose}
          />
        </header>
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
                <dd>
                  <code>{detail.sandbox.owner}</code>
                </dd>
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
                      <code>{workspace.id}</code>
                    </li>
                  ))}
                </ul>
              )}
            </section>
            <section>
              <h3>Agents ({detail.agents.length})</h3>
              {detail.agents.length === 0 ? (
                <p>No Agents.</p>
              ) : (
                <ul>
                  {detail.agents.map((agent) => (
                    <li key={agent.id}>
                      <p>
                        {agent.title ?? agent.id}: {stateWord(agent.status)}
                      </p>
                      <p>
                        Workspace <code>{agent.workspace_id}</code>
                      </p>
                      <code>{agent.worktree_id}</code>
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
    </div>,
    document.body,
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
