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
import { IconPauseOutline16, IconPlayOutline16, IconPlusOutline16 } from '../ui/icons';
import { MoreActions } from '../ui/MoreActions';
import { sandboxCreateReason, sandboxHeaderMenu } from './sandboxHeaderActions';
import {
  sandboxLifecycleAction,
  sandboxLifecycleLabel,
  sandboxRefreshDelay,
  sandboxTone,
} from './lifecycle';
import { Badge } from '../ui/Badge';
import { SandboxRuntimeUpdate } from './SandboxRuntimeUpdate';
import { SandboxAdvanced } from './SandboxAdvanced';
import { SandboxAlerts } from './SandboxAlerts';
import { SandboxSections } from './SandboxSections';
import { readSandboxTab, rememberSandboxTab, type SandboxTab } from './sandboxTabRoute';
import './SandboxPage.css';
import { ConnectionWizard } from '../connections/ConnectionWizard';
import { SharingDialog, useSharingAllowed } from '../resources/SharingDialog';
import type { ResourceViewsClient } from '../resources/client';
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
  const sharingClient = stale ? undefined : client.resourceViews;
  const sharingAllowed = useSharingAllowed(sharingClient, { kind: 'sandbox', id: sandboxId });
  // Opened for one client, so a changed or withdrawn client closes the dialog.
  const [sharingFor, setSharingFor] = useState<ResourceViewsClient>();
  // Read once: the URL names the tab a refresh or a link asked for.
  const [chosenTab, setChosenTab] = useState(() => readSandboxTab(window.location.search));
  // Without one, a Sandbox still being prepared opens where its progress is.
  const [firstTab, setFirstTab] = useState<SandboxTab>();
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
  const createReasonId = useId();
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
  const lifecycleReason = permissions.pauseSandbox ? lifecycleAction.reason : undefined;
  const actionContext = { busy, stale, needsConnections };
  const childReason = sandboxCreateReason(sandbox, actionContext);
  const createReason = permissions.createAgent ? childReason : undefined;
  if (firstTab === undefined)
    setFirstTab(['provisioning', 'installing'].includes(sandbox.status) ? 'activity' : 'overview');
  const tab = chosenTab ?? firstTab ?? 'overview';
  const selectTab = (next: SandboxTab) => {
    setChosenTab(next);
    rememberSandboxTab(sandboxId, next);
  };
  const runtime =
    sandbox.runtimeUpdate !== undefined && viewerId !== undefined && sandbox.owner === viewerId ? (
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
    ) : undefined;
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
            {permissions.createAgent ? (
              <Button
                size="sm"
                variant="primary"
                icon={<IconPlusOutline16 />}
                disabled={createReason !== undefined}
                aria-describedby={
                  createReason === undefined
                    ? undefined
                    : createReason === lifecycleReason
                      ? lifecycleReasonId
                      : createReasonId
                }
                onClick={() => onAction({ kind: 'new_agent', sandboxId })}
                aria-label="New agent in this Sandbox"
              >
                New agent
              </Button>
            ) : null}
            {permissions.pauseSandbox ? (
              <Button
                size="sm"
                variant="outline"
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
            <MoreActions
              items={sandboxHeaderMenu(sandbox, permissions, actionContext, sharingAllowed)}
              onSelect={(id) => {
                if (id === 'new_workspace') onAction({ kind: 'new_workspace', sandboxId });
                else if (id === 'delete_sandbox') onAction({ kind: 'delete_sandbox', sandboxId });
                else if (id === 'sharing') setSharingFor(sharingClient);
                else if (id === 'technical_details') setAdvanced(true);
              }}
            />
          </>
        }
      />
      {lifecycleReason === undefined && createReason === undefined ? null : (
        <div className="sandbox-page__reasons">
          {lifecycleReason === undefined ? null : <p id={lifecycleReasonId}>{lifecycleReason}</p>}
          {createReason === undefined || createReason === lifecycleReason ? null : (
            <p id={createReasonId}>{createReason}</p>
          )}
        </div>
      )}
      {needsConnections && !stale ? (
        <ConnectionWizard
          target={{ type: 'sandbox', id: sandboxId }}
          presentation="dialog"
          expectedOwner={viewerId}
          onReady={() => setConnectionScope({ client, sandboxId, viewerId })}
          onCancel={() => onNavigate({ kind: 'home' })}
        />
      ) : null}
      <SandboxAlerts
        sandbox={sandbox}
        pollError={pollError}
        stale={stale}
        onViewProgress={tab === 'activity' ? undefined : () => selectTab('activity')}
      />
      <SandboxSections
        key={`${sandbox.id}:${sandbox.owner ?? ''}:${JSON.stringify(sandbox.profile)}`}
        client={client}
        sandbox={sandbox}
        observedAt={state.observedAt}
        permissions={permissions}
        busy={busy || stale}
        stale={stale}
        readEnabled={!accessUncertain}
        createReason={childReason}
        runtime={runtime}
        selected={tab}
        onSelect={selectTab}
        onAction={onAction}
        onNavigate={onNavigate}
      />
      {client.resourceViews === undefined ? null : (
        <SharingDialog
          key={sandboxId}
          client={client.resourceViews}
          target={{ kind: 'sandbox', id: sandboxId, name: sandbox.name }}
          open={sharingFor !== undefined && sharingFor === sharingClient}
          onClose={() => setSharingFor(undefined)}
        />
      )}
      <Presence>
        {advanced ? (
          <SandboxAdvanced client={client} sandbox={sandbox} onClose={() => setAdvanced(false)} />
        ) : null}
      </Presence>
    </section>
  );
}
