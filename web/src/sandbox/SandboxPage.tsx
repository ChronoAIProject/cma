import { useEffect, useRef, useState } from 'react';
import type { CmaWebClient } from '../api/client';
import type { SandboxDetail, SandboxEvent, WorkspaceDetail } from '../api/types';
import type { AppRoute } from '../routing/routes';
import { routePath } from '../routing/routes';
import type { TreeAction, TreePermissions } from '../shell/Tree';
import { Button } from '../ui/Button';
import { IconPauseOutline16, IconPlayOutline16, IconTrashOutline16 } from '../ui/icons';
import './SandboxPage.css';

type State =
  | { status: 'loading' }
  | { status: 'error'; message: string }
  | {
      status: 'ready';
      sandbox: SandboxDetail;
      workspaces: readonly WorkspaceDetail[];
      events: readonly SandboxEvent[];
    };

async function load(client: CmaWebClient, id: string, signal: AbortSignal): Promise<State> {
  const sandbox = await client.readSandbox(id, { signal });
  if (sandbox.id !== id) throw new Error('Sandbox projection referred to another resource.');
  const workspaces = await Promise.all(
    sandbox.workspaces.map((workspace) => client.readWorkspace(workspace.id, { signal })),
  );
  const events: SandboxEvent[] = [];
  let cursor = 0;
  for (;;) {
    const page = await client.readSandboxEvents(id, { cursor, signal });
    if (page.length === 0) break;
    if (page.some((event) => event.sandboxId !== id || event.seq <= cursor))
      throw new Error('Sandbox events did not advance.');
    events.push(...page);
    cursor = Math.max(...page.map((event) => event.seq));
  }
  return { status: 'ready', sandbox, workspaces, events };
}

export function SandboxPage({
  client,
  sandboxId,
  revision,
  permissions,
  busy,
  onAction,
  onNavigate,
}: {
  readonly client: CmaWebClient;
  readonly sandboxId: string;
  readonly revision: number;
  readonly permissions: TreePermissions;
  readonly busy: boolean;
  readonly onAction: (action: TreeAction) => void;
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const [state, setState] = useState<State>({ status: 'loading' });
  const [retry, setRetry] = useState(0);
  const heading = useRef<HTMLHeadingElement>(null);
  const documentTitle = state.status === 'ready' ? state.sandbox.name : 'Sandbox';
  useEffect(() => {
    const abort = new AbortController();
    void load(client, sandboxId, abort.signal)
      .then((next) => {
        if (!abort.signal.aborted) setState(next);
      })
      .catch((error: unknown) => {
        if (!abort.signal.aborted)
          setState({
            status: 'error',
            message: error instanceof Error ? error.message : 'Sandbox unavailable.',
          });
      });
    return () => abort.abort();
  }, [client, sandboxId, revision, retry]);
  useEffect(() => {
    heading.current?.focus();
    document.title = `${documentTitle} · CMA`;
    return () => {
      document.title = 'CMA';
    };
  }, [sandboxId, state.status, documentTitle]);
  if (state.status === 'loading')
    return (
      <section className="sandbox-page" aria-busy="true">
        <h1 ref={heading} tabIndex={-1}>
          Sandbox
        </h1>
      </section>
    );
  if (state.status === 'error')
    return (
      <section className="sandbox-page" role="alert">
        <h1 ref={heading} tabIndex={-1}>
          Sandbox unavailable
        </h1>
        <p>{state.message}</p>
        <Button onClick={() => setRetry((value) => value + 1)}>Try again</Button>
      </section>
    );
  const { sandbox, workspaces, events } = state;
  return (
    <section className="sandbox-page">
      <header className="sandbox-page__header">
        <div>
          <h1 ref={heading} tabIndex={-1}>
            {sandbox.name}
          </h1>
          <p>{sandbox.status.replaceAll('_', ' ')}</p>
        </div>
        <div className="sandbox-page__actions">
          {permissions.pauseSandbox ? (
            <Button
              icon={sandbox.status === 'paused' ? <IconPlayOutline16 /> : <IconPauseOutline16 />}
              disabled={busy || !['paused', 'active'].includes(sandbox.status)}
              onClick={() =>
                onAction({
                  kind: sandbox.status === 'paused' ? 'revive_sandbox' : 'pause_sandbox',
                  sandboxId,
                })
              }
            >
              {sandbox.status === 'paused' ? 'Revive' : 'Pause'}
            </Button>
          ) : null}
          {permissions.deleteSandbox ? (
            <Button
              variant="danger"
              icon={<IconTrashOutline16 />}
              disabled={busy || ['retiring', 'retired'].includes(sandbox.status)}
              onClick={() => onAction({ kind: 'delete_sandbox', sandboxId })}
            >
              Delete
            </Button>
          ) : null}
        </div>
      </header>
      <section className="sandbox-page__band">
        <h2>Profile</h2>
        <dl>
          <div>
            <dt>Profile</dt>
            <dd>
              {sandbox.profile.id} @ {sandbox.profile.revision}
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
          {sandbox.runtimeVersion === undefined ? null : (
            <div>
              <dt>Runtime version</dt>
              <dd>
                <code>{sandbox.runtimeVersion}</code>
              </dd>
            </div>
          )}
        </dl>
      </section>
      {sandbox.failure === undefined ? null : (
        <p role="alert">
          {sandbox.failure.detail} <code>{sandbox.failure.class}</code>
        </p>
      )}
      <section className="sandbox-page__band">
        <h2>Workspaces</h2>
        {workspaces.length === 0 ? (
          <p>No workspaces</p>
        ) : (
          workspaces.map((workspace) => (
            <section className="sandbox-page__workspace" key={workspace.id}>
              <h3>{workspace.name}</h3>
              <p>
                <code>{workspace.path}</code>
              </p>
              <p>{workspace.status}</p>
              <ul>
                {workspace.agents.map((agent) => {
                  const route = { kind: 'agent', agentId: agent.id } as const;
                  return (
                    <li key={agent.id}>
                      <a
                        href={routePath(route)}
                        onClick={(event) => {
                          if (
                            event.button !== 0 ||
                            event.metaKey ||
                            event.ctrlKey ||
                            event.shiftKey ||
                            event.altKey
                          )
                            return;
                          event.preventDefault();
                          onNavigate(route);
                        }}
                      >
                        {agent.title?.trim() || 'Untitled agent'}
                      </a>
                      <span>
                        {sandbox.status === 'paused' ? 'Paused' : agent.status.replaceAll('_', ' ')}
                      </span>
                    </li>
                  );
                })}
              </ul>
            </section>
          ))
        )}
      </section>
      <section className="sandbox-page__band">
        <h2>Events</h2>
        {events.length === 0 ? (
          <p>No events</p>
        ) : (
          <ol className="sandbox-page__events">
            {events.map((event) => (
              <li key={event.seq}>
                <span>{event.kind.type.replaceAll('_', ' ')}</span>
                <time dateTime={event.appendedAt}>
                  {new Date(event.appendedAt).toLocaleString()}
                </time>
              </li>
            ))}
          </ol>
        )}
      </section>
    </section>
  );
}
