import { Presence } from '../ui/Presence';
import { SkeletonRows } from '../ui/Skeleton';
import { useEffect, useId, useMemo, useRef, useState } from 'react';
import { ApiError } from '@chronoai/cma-sdk';
import type { CmaWebClient } from '../api/client';
import type { SandboxDetail } from '../api/types';
import { routePath, type AppRoute } from '../routing/routes';
import { PageHeader } from '../ui/PageHeader';
import { useDocumentTitle } from '../ui/useDocumentTitle';
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
  sandboxTone,
} from './lifecycle';
import { Badge } from '../ui/Badge';
import { Alert } from '../ui/Alert';
import { SandboxOverview } from './SandboxOverview';
import { SandboxRuntimeUpdate } from './SandboxRuntimeUpdate';
import { SandboxAdvanced } from './SandboxAdvanced';
import { LifecycleProgress } from './LifecycleProgress';
import { SandboxRecoveryNotice, sandboxRecoveryFailed } from './SandboxRecoveryNotice';
import './SandboxPage.css';
import { ConnectionWizard } from '../connections/ConnectionWizard';
import { SharingButton } from '../resources/SharingButton';
import { useOwnerViews } from '../resources/retainedOwnerViews';

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
  accessUncertain = false,
}: {
  readonly client: CmaWebClient;
  readonly sandboxId: string;
  readonly revision: number;
  readonly permissions: TreePermissions;
  readonly busy: boolean;
  readonly onAction: (action: TreeAction) => void;
  readonly onNavigate: (route: AppRoute) => void;
  readonly viewerId?: string;
  readonly accessUncertain?: boolean;
}) {
  const views = useOwnerViews();
  const [retry, setRetry] = useState(0);
  const request = useMemo(
    () => ({ client, sandboxId, revision, retry, views, viewerId }),
    [client, sandboxId, revision, retry, views, viewerId],
  );
  const [stored, setStored] = useState<{
    request: typeof request;
    state: State;
    pollError: boolean;
  }>();
  const retained =
    views?.client === client && views.viewerId === viewerId
      ? views.sandboxes.get(sandboxId)
      : undefined;
  const state: State =
    stored?.request === request
      ? stored.state
      : retained === undefined
        ? { status: 'loading' }
        : { status: 'ready', ...retained };
  const pollError = stored?.request === request && stored.pollError;
  const revalidating = stored?.request !== request || accessUncertain;
  const stale =
    pollError ||
    revalidating ||
    (state.status === 'ready' &&
      viewerId !== undefined &&
      state.sandbox.owner !== undefined &&
      state.sandbox.owner !== viewerId);
  const [advanced, setAdvanced] = useState(false);
  const [progressOpen, setProgressOpen] = useState(false);
  const [connectionScope, setConnectionScope] = useState<{
    client: CmaWebClient;
    sandboxId: string;
    viewerId?: string;
  }>();
  const connectionsReady =
    connectionScope?.client === client &&
    connectionScope?.sandboxId === sandboxId &&
    connectionScope?.viewerId === viewerId;
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
          const value = { sandbox, observedAt: new Date().toISOString() };
          if (
            views?.client === client &&
            sandbox.owner === views.viewerId &&
            !['retiring', 'retired'].includes(sandbox.status)
          ) {
            const cached = views.sandboxes.get(sandboxId);
            if (cached === undefined || cached.sandbox.revision <= sandbox.revision)
              views.sandboxes.put(sandboxId, value);
          } else views?.forgetSandbox(sandboxId);
          setStored((previous) =>
            previous?.request === request &&
            previous.state.status === 'ready' &&
            previous.state.sandbox.owner === sandbox.owner &&
            previous.state.sandbox.revision > sandbox.revision
              ? { ...previous, pollError: false }
              : { request, state: { status: 'ready', ...value }, pollError: false },
          );
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
          const denied = error instanceof ApiError && [401, 403, 404, 410].includes(error.status);
          if (denied) views?.forgetSandbox(sandboxId);
          setStored((previous) => {
            const current = previous?.request === request ? previous.state : undefined;
            const cached =
              views?.client === client && views.viewerId === viewerId
                ? views.sandboxes.get(sandboxId)
                : undefined;
            return {
              request,
              pollError: true,
              state:
                !denied && current?.status === 'ready'
                  ? current
                  : !denied && cached !== undefined
                    ? { status: 'ready', ...cached }
                    : {
                        status: 'error',
                        message: error instanceof Error ? error.message : 'Sandbox unavailable.',
                      },
            };
          });
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
  }, [client, sandboxId, request, views, viewerId]);
  useDocumentTitle(documentTitle);
  useEffect(() => {
    heading.current?.focus();
  }, [sandboxId, state.status]);
  if (state.status === 'loading')
    return (
      <section className="sandbox-page" aria-busy="true">
        <h1 ref={heading} tabIndex={-1}>
          Sandbox
        </h1>
        <SkeletonRows rows={4} label="Loading Sandbox" />
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
      stale={stale}
    />
  );
  return (
    <section className="sandbox-page">
      <PageHeader
        breadcrumb={[
          {
            label: 'Agent Manager',
            href: routePath({ kind: 'agent_manager' }),
            onSelect: () => onNavigate({ kind: 'agent_manager' }),
          },
          { label: sandbox.name },
        ]}
        title={sandbox.name}
        headingRef={heading}
        badge={
          <Badge tone={sandboxTone(sandbox)} live>
            {sandboxLifecycleLabel(sandbox)}
          </Badge>
        }
        meta={<span className="page-header__chip">{sandbox.id}</span>}
        actions={
          <>
            {client.resourceViews === undefined || stale ? null : (
              <SharingButton
                client={client.resourceViews}
                target={{ kind: 'sandbox', id: sandboxId, name: sandbox.name }}
              />
            )}
            {permissions.createAgent ? (
              <Button
                icon={<IconPlusOutline16 />}
                disabled={busy || stale || needsConnections || !childrenAvailable}
                onClick={() => onAction({ kind: 'new_agent', sandboxId })}
                aria-label="New agent in this Sandbox"
              >
                New agent
              </Button>
            ) : null}
            {permissions.createWorkspace ? (
              <Button
                icon={<IconPlusOutline16 />}
                disabled={busy || stale || needsConnections || !childrenAvailable}
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
                  stale ||
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
                  stale ||
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
          </>
        }
      />
      {needsConnections && !stale ? (
        <ConnectionWizard
          target={{ type: 'sandbox', id: sandboxId }}
          presentation="dialog"
          expectedOwner={viewerId}
          onReady={() => setConnectionScope({ client, sandboxId, viewerId })}
          onCancel={() => onNavigate({ kind: 'home' })}
        />
      ) : null}
      <div className="alert-stack">
        <Presence>
          {pollError ? (
            <Alert tone="warning">Progress could not be refreshed. Reconnecting.</Alert>
          ) : null}
        </Presence>
        {notice === undefined ? null : <Alert tone="info">{notice}</Alert>}
      </div>
      <SandboxRecoveryNotice sandbox={sandbox} stale={stale} showHistory />
      {permissions.pauseSandbox && lifecycleAction.reason !== undefined ? (
        <p id={lifecycleReasonId}>{lifecycleAction.reason}</p>
      ) : null}
      {optionalProgress ? (
        <details onToggle={(event) => setProgressOpen(event.currentTarget.open)}>
          <summary>Sandbox operation progress</summary>
          {progressOpen ? progress : null}
        </details>
      ) : (
        progress
      )}
      <Presence>
        {sandbox.status === 'failed' && !sandboxRecoveryFailed(sandbox) ? (
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
      {sandbox.runtimeUpdate !== undefined &&
      viewerId !== undefined &&
      sandbox.owner === viewerId ? (
        <SandboxRuntimeUpdate
          client={client}
          sandboxId={sandboxId}
          ownerId={viewerId}
          revision={sandbox.revision}
          update={sandbox.runtimeUpdate}
          stale={stale}
          busy={busy}
          onAccepted={(receipt) => {
            const cached = views?.sandboxes.get(sandboxId);
            if (cached !== undefined && cached.sandbox.revision <= receipt.revision)
              views?.sandboxes.put(sandboxId, {
                ...cached,
                sandbox: { ...cached.sandbox, ...receipt },
              });
            setStored((previous) => {
              if (
                previous?.request !== request ||
                previous.state.status !== 'ready' ||
                previous.state.sandbox.owner !== viewerId ||
                previous.state.sandbox.revision > receipt.revision
              )
                return previous;
              return {
                ...previous,
                state: { ...previous.state, sandbox: { ...previous.state.sandbox, ...receipt } },
              };
            });
          }}
        />
      ) : null}
      <SandboxOverview
        key={`${sandbox.id}:${sandbox.owner ?? ''}:${JSON.stringify(sandbox.profile)}`}
        stale={stale}
        client={client}
        sandbox={sandbox}
        permissions={permissions}
        busy={busy || stale}
        readEnabled={!accessUncertain}
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
