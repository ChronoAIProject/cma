import { useEffect, useRef, useState } from 'react';
import type { CmaWebClient } from '../api/client';
import type { SandboxDetail, SandboxEvent, WorkspaceDetail } from '../api/types';
import type { AppRoute } from '../routing/routes';
import { routePath } from '../routing/routes';
import type { TreeAction, TreePermissions } from '../shell/Tree';
import { Button } from '../ui/Button';
import {
  IconPauseOutline16,
  IconPlayOutline16,
  IconPlusOutline16,
  IconTrashOutline16,
} from '../ui/icons';
import { ProvisioningProgress } from './ProvisioningProgress';
import { sandboxProgress, workspaceProgress } from './progress';
import { readSandboxHistory } from './eventHistory';
import { eventLabel, statusLabel } from '../vocabulary/status';
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

async function load(
  client: CmaWebClient,
  id: string,
  signal: AbortSignal,
  previous: readonly SandboxEvent[] = [],
): Promise<Extract<State, { status: 'ready' }>> {
  const sandbox = await client.readSandbox(id, { signal });
  if (sandbox.id !== id) throw new Error('Sandbox projection referred to another resource.');
  const workspaces = await Promise.all(
    sandbox.workspaces.map((workspace) => client.readWorkspace(workspace.id, { signal })),
  );
  const events = await readSandboxHistory(client, id, signal, previous);
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
  const [pollError, setPollError] = useState(false);
  const heading = useRef<HTMLHeadingElement>(null);
  const documentTitle = state.status === 'ready' ? state.sandbox.name : 'Sandbox';
  useEffect(() => {
    const abort = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let previous: readonly SandboxEvent[] = [];
    let loaded = false;
    const poll = () => {
      void load(client, sandboxId, abort.signal, previous)
        .then((next) => {
          if (abort.signal.aborted) return;
          loaded = true;
          previous = next.events;
          setState(next);
          setPollError(false);
          if (
            [
              'provisioning',
              'installing',
              'reviving',
              'rotating',
              'credential_rotating',
              'retiring',
              'pausing',
            ].includes(next.sandbox.status) ||
            next.workspaces.some(
              (workspace) => workspace.status === 'preparing' || workspace.status === 'removing',
            )
          )
            timer = setTimeout(poll, 1500);
        })
        .catch((error: unknown) => {
          if (abort.signal.aborted) return;
          if (loaded) {
            setPollError(true);
            timer = setTimeout(poll, 5000);
          } else
            setState({
              status: 'error',
              message: error instanceof Error ? error.message : 'Sandbox unavailable.',
            });
        });
    };
    poll();
    return () => {
      abort.abort();
      clearTimeout(timer);
    };
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
          <p>{statusLabel(sandbox.status)}</p>
        </div>
        <div className="sandbox-page__actions">
          {permissions.createWorkspace ? (
            <Button
              icon={<IconPlusOutline16 />}
              disabled={busy || !['active', 'paused', 'reviving'].includes(sandbox.status)}
              onClick={() => onAction({ kind: 'new_workspace', sandboxId })}
            >
              New workspace
            </Button>
          ) : null}
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
              {sandbox.status === 'paused' ? 'Resume' : 'Pause'}
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
      {pollError ? <p role="alert">Progress could not be refreshed. Reconnecting.</p> : null}
      {['provisioning', 'installing', 'failed'].includes(sandbox.status) ? (
        <ProvisioningProgress
          title="Sandbox preparation"
          steps={sandboxProgress(sandbox, events)}
          failure={sandbox.failure}
        />
      ) : null}
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
              <p>{statusLabel(workspace.status)}</p>
              {['preparing', 'failed'].includes(workspace.status) ? (
                <ProvisioningProgress
                  title={`${workspace.name} preparation`}
                  steps={workspaceProgress(workspace, events)}
                  failure={workspace.failure}
                />
              ) : null}
              {permissions.createAgent ? (
                <Button
                  icon={<IconPlusOutline16 />}
                  disabled={
                    busy ||
                    !['active', 'paused', 'reviving'].includes(sandbox.status) ||
                    workspace.status !== 'ready'
                  }
                  onClick={() => onAction({ kind: 'new_agent', workspaceId: workspace.id })}
                >
                  New agent
                </Button>
              ) : null}
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
                        {sandbox.status === 'paused' ? 'Paused' : statusLabel(agent.status)}
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
                <span>{eventLabel(event.kind.type)}</span>
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
