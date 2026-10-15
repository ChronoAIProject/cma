import { Presence } from '../ui/Presence';
import { useEffect, useId, useRef, useState } from 'react';
import { ApiError } from '@chronoai/cma-sdk';
import type { CmaWebClient } from '../api/client';
import type { SandboxDetail } from '../api/types';
import type { AppRoute } from '../routing/routes';
import type { TreeAction, TreePermissions } from '../shell/Tree';
import { Button } from '../ui/Button';
import {
  IconPauseOutline16,
  IconPlayOutline16,
  IconPlusOutline16,
  IconSettingsOutline16,
  IconTrashOutline16,
} from '../ui/icons';
import {
  sandboxChildrenAvailable,
  sandboxUnavailableNotice,
  sandboxLifecycleAction,
  sandboxLifecycleLabel,
  sandboxRefreshDelay,
} from './lifecycle';
import { SandboxOverview } from './SandboxOverview';
import { SandboxAdvanced } from './SandboxAdvanced';
import { LifecycleProgress } from './LifecycleProgress';
import './SandboxPage.css';
import { ConnectionWizard } from '../connections/ConnectionWizard';
import { SharingButton } from '../resources/SharingButton';

type State =
  | { status: 'loading' }
  | { status: 'error'; message: string }
  | { status: 'ready'; sandbox: SandboxDetail; observedAt: string };

export function SandboxPage({
  client,
  sandboxId,
  revision,
  permissions,
  busy,
  onAction,
  onNavigate,
  viewerId,
}: {
  readonly client: CmaWebClient;
  readonly sandboxId: string;
  readonly revision: number;
  readonly permissions: TreePermissions;
  readonly busy: boolean;
  readonly onAction: (action: TreeAction) => void;
  readonly onNavigate: (route: AppRoute) => void;
  readonly viewerId?: string;
}) {
  const [state, setState] = useState<State>({ status: 'loading' });
  const [retry, setRetry] = useState(0);
  const [pollError, setPollError] = useState(false);
  const [advanced, setAdvanced] = useState(false);
  const [progressOpen, setProgressOpen] = useState(false);
  const [connectionsReady, setConnectionsReady] = useState(false);
  const lifecycleReasonId = useId();
  const heading = useRef<HTMLHeadingElement>(null);
  const documentTitle = state.status === 'ready' ? state.sandbox.name : 'Sandbox';
  useEffect(() => {
    const abort = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let inFlight = false;
    const poll = () => {
      if (abort.signal.aborted || inFlight || document.hidden) return;
      clearTimeout(timer);
      inFlight = true;
      void client
        .readSandbox(sandboxId, { signal: abort.signal })
        .then((sandbox) => {
          if (abort.signal.aborted) return;
          if (sandbox.id !== sandboxId)
            throw new Error('Sandbox projection referred to another resource.');
          setState({ status: 'ready', sandbox, observedAt: new Date().toISOString() });
          setPollError(false);
          timer = setTimeout(
            poll,
            sandbox.workspaces.some(
              (workspace) => workspace.status === 'preparing' || workspace.status === 'removing',
            )
              ? 1500
              : sandboxRefreshDelay(sandbox),
          );
        })
        .catch((error: unknown) => {
          if (abort.signal.aborted) return;
          setPollError(true);
          const denied = error instanceof ApiError && [401, 403, 404].includes(error.status);
          setState((current) =>
            !denied && current.status === 'ready' && current.sandbox.id === sandboxId
              ? current
              : {
                  status: 'error',
                  message: error instanceof Error ? error.message : 'Sandbox unavailable.',
                },
          );
          timer = setTimeout(poll, 5000);
        })
        .finally(() => {
          inFlight = false;
        });
    };
    document.addEventListener('visibilitychange', poll);
    poll();
    return () => {
      abort.abort();
      clearTimeout(timer);
      document.removeEventListener('visibilitychange', poll);
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
  const { sandbox } = state;
  const needsConnections =
    viewerId !== undefined && sandbox.owner === viewerId && !connectionsReady;
  const lifecycleAction = sandboxLifecycleAction(sandbox, pollError);
  const childrenAvailable = sandboxChildrenAvailable(sandbox);
  const notice = sandboxUnavailableNotice(sandbox);
  const optionalProgress =
    sandbox.lifecycle?.operation !== undefined ||
    [
      'paused',
      'pausing',
      'reviving',
      'rotating',
      'credential_rotating',
      'retiring',
      'retired',
    ].includes(sandbox.status);
  const progress = (
    <LifecycleProgress
      client={client}
      sandbox={sandbox}
      observedAt={state.observedAt}
      stale={pollError}
    />
  );
  return (
    <section className="sandbox-page">
      <header className="sandbox-page__header">
        <div>
          <h1 ref={heading} tabIndex={-1}>
            {sandbox.name}
          </h1>
          <p role="status">{sandboxLifecycleLabel(sandbox)}</p>
        </div>
        <div className="sandbox-page__actions">
          {client.resourceViews === undefined ? null : (
            <SharingButton
              client={client.resourceViews}
              target={{ kind: 'sandbox', id: sandboxId, name: sandbox.name }}
            />
          )}
          {permissions.createAgent ? (
            <Button
              icon={<IconPlusOutline16 />}
              disabled={busy || pollError || needsConnections || !childrenAvailable}
              onClick={() => onAction({ kind: 'new_agent', sandboxId })}
              aria-label="New agent in this Sandbox"
            >
              New agent
            </Button>
          ) : null}
          {permissions.createWorkspace ? (
            <Button
              icon={<IconPlusOutline16 />}
              disabled={busy || pollError || needsConnections || !childrenAvailable}
              onClick={() => onAction({ kind: 'new_workspace', sandboxId })}
            >
              New workspace
            </Button>
          ) : null}
          {permissions.pauseSandbox ? (
            <Button
              icon={
                lifecycleAction.kind === 'revive_sandbox' ? (
                  <IconPlayOutline16 />
                ) : (
                  <IconPauseOutline16 />
                )
              }
              disabled={
                busy ||
                !lifecycleAction.allowed ||
                (needsConnections && lifecycleAction.kind === 'revive_sandbox')
              }
              aria-describedby={
                lifecycleAction.reason === undefined ? undefined : lifecycleReasonId
              }
              onClick={() => onAction({ kind: lifecycleAction.kind, sandboxId })}
            >
              {lifecycleAction.label}
            </Button>
          ) : null}
          {permissions.deleteSandbox ? (
            <Button
              variant="danger"
              icon={<IconTrashOutline16 />}
              disabled={
                busy ||
                pollError ||
                ['retiring', 'retired'].includes(sandbox.status) ||
                sandbox.lifecycle?.checkpoint !== undefined ||
                sandbox.lifecycle?.operation !== undefined
              }
              onClick={() => onAction({ kind: 'delete_sandbox', sandboxId })}
            >
              Delete
            </Button>
          ) : null}
          <Button icon={<IconSettingsOutline16 />} onClick={() => setAdvanced(true)}>
            Advanced
          </Button>
        </div>
      </header>
      {needsConnections ? (
        <ConnectionWizard
          target={{ type: 'sandbox', id: sandboxId }}
          presentation="dialog"
          expectedOwner={viewerId}
          onReady={() => setConnectionsReady(true)}
          onCancel={() => onNavigate({ kind: 'home' })}
        />
      ) : null}
      {notice === undefined ? null : <p>{notice}</p>}
      {permissions.pauseSandbox && lifecycleAction.reason !== undefined ? (
        <p id={lifecycleReasonId}>{lifecycleAction.reason}</p>
      ) : null}
      <Presence>
        {pollError ? <p role="alert">Progress could not be refreshed. Reconnecting.</p> : null}
      </Presence>
      {optionalProgress ? (
        <details onToggle={(event) => setProgressOpen(event.currentTarget.open)}>
          <summary>Sandbox operation progress</summary>
          {progressOpen ? progress : null}
        </details>
      ) : (
        progress
      )}
      <Presence>
        {sandbox.status === 'failed' ? (
          <p role="alert">
            Your sandbox needs attention.{' '}
            {sandbox.failure?.remediation ?? 'Open Advanced for the reported failure.'}
          </p>
        ) : null}
      </Presence>
      <dl>
        <div>
          <dt>Owner</dt>
          <dd>{sandbox.ownerName?.trim() || 'Name unavailable'}</dd>
        </div>
      </dl>
      <SandboxOverview
        stale={pollError}
        client={client}
        sandbox={sandbox}
        permissions={permissions}
        busy={busy || pollError}
        onAction={onAction}
        onNavigate={onNavigate}
      />
      <Presence>
        {advanced ? (
          <SandboxAdvanced client={client} sandbox={sandbox} onClose={() => setAdvanced(false)} />
        ) : null}
      </Presence>
    </section>
  );
}
