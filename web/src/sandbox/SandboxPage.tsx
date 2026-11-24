import { Presence } from '../ui/Presence';
import { SkeletonRows } from '../ui/Skeleton';
import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { ApiError } from '@cma/client';
import type { CmaWebClient } from '../api/client';
import { detailReadSource } from '../api/detailReads';
import { createPagePollDelay, sandboxPollKey } from './pollDelay';
import type { SandboxDetail } from '../api/types';
import { routePath, type AppRoute } from '../routing/routes';
import { PageHeader } from '../ui/PageHeader';
import { useDocumentTitle } from '../ui/useDocumentTitle';
import type { TreeAction, TreePermissions } from '../shell/Tree';
import { Button } from '../ui/Button';
import { IconPauseOutline16, IconPlayOutline16 } from '../ui/icons';
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
import { sandboxHasParkedRuntimeUpdate } from './parkedRuntimeUpdate';
import { SandboxRetainedRecovery } from './SandboxRetainedRecovery';
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

const POLL_ERROR_DELAY = 5000;

export function SandboxPage({
  client,
  sandboxId,
  revision,
  permissions,
  busy: submitting,
  lifecycleBusy = false,
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
  readonly lifecycleBusy?: boolean;
  readonly onAction: (action: TreeAction, eligibility?: AbortSignal) => void;
  readonly onNavigate: (route: AppRoute) => void;
  readonly viewerId?: string;
  readonly accessUncertain?: boolean;
}) {
  const busy = submitting || lifecycleBusy;
  const views = useOwnerViews();
  const [retry, setRetry] = useState(0);
  const pollScope = useMemo(() => ({ client, sandboxId, viewerId }), [client, sandboxId, viewerId]);
  const request = useMemo(
    () => ({ pollScope, revision, retry, views }),
    [pollScope, revision, retry, views],
  );
  const retryFloor = useRef<{ scope: typeof pollScope; at: number }>(undefined);
  const pollNow = useRef<() => void>(undefined);
  const [stored, setStored] = useState<{
    request: typeof request;
    state: State;
    pollError: boolean;
  }>();
  const retained =
    views?.client === detailReadSource(client) && views.viewerId === viewerId
      ? views.sandboxes.get(sandboxId)
      : undefined;
  const previous = stored?.request.pollScope === pollScope ? stored : undefined;
  const state: State =
    previous?.request === request
      ? previous.state
      : retained !== undefined
        ? { status: 'ready', ...retained }
        : previous?.state.status === 'ready'
          ? previous.state
          : { status: 'loading' };
  const pollError = previous?.pollError ?? false;
  const revalidating = stored?.request !== request || accessUncertain;
  const stale =
    pollError ||
    revalidating ||
    (state.status === 'ready' &&
      viewerId !== undefined &&
      state.sandbox.owner !== undefined &&
      state.sandbox.owner !== viewerId);
  const canResume =
    !stale &&
    permissions.pauseSandbox &&
    state.status === 'ready' &&
    state.sandbox.status === 'paused' &&
    state.sandbox.lifecycle?.revive.allowed === true;
  const [resumeCheck, setResumeCheck] = useState<{
    request: typeof request;
    controller: AbortController;
  }>();
  useLayoutEffect(() => {
    if (!canResume || resumeCheck?.request !== request) resumeCheck?.controller.abort();
    return () => resumeCheck?.controller.abort();
  }, [canResume, request, resumeCheck]);
  const [advanced, setAdvanced] = useState(false);
  const runtimePanel = useRef<HTMLDivElement>(null);
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
    // A revision refresh restarts the effect while the same Sandbox still owes the wait.
    const floor =
      retryFloor.current?.scope === pollScope ? retryFloor.current : { scope: pollScope, at: 0 };
    retryFloor.current = floor;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let inFlight = false;
    const delay = createPagePollDelay();
    const poll = () => {
      if (abort.signal.aborted || inFlight || document.hidden) return;
      clearTimeout(timer);
      const remaining = floor.at - performance.now();
      if (remaining > 0) {
        timer = setTimeout(poll, Math.ceil(remaining));
        return;
      }
      inFlight = true;
      void client
        .readSandbox(sandboxId, { signal: abort.signal })
        .then((sandbox) => {
          if (abort.signal.aborted) return;
          if (sandbox.id !== sandboxId)
            throw new Error('Sandbox projection referred to another resource.');
          floor.at = 0;
          const value = { sandbox, observedAt: new Date().toISOString() };
          if (
            views?.client === detailReadSource(client) &&
            sandbox.owner === views.viewerId &&
            !['retiring', 'retired'].includes(sandbox.status)
          ) {
            const cached = views.sandboxes.get(sandboxId);
            if (cached === undefined || cached.sandbox.revision <= sandbox.revision)
              views.sandboxes.put(sandboxId, value);
          } else views?.forgetSandbox(sandboxId);
          setStored((previous) =>
            previous?.request.pollScope === pollScope &&
            previous.state.status === 'ready' &&
            previous.state.sandbox.owner === sandbox.owner &&
            previous.state.sandbox.revision > sandbox.revision
              ? {
                  ...previous,
                  request,
                  pollError: previous.pollError || previous.request !== request,
                }
              : { request, state: { status: 'ready', ...value }, pollError: false },
          );
          timer = setTimeout(
            poll,
            delay.next(
              JSON.stringify([
                sandboxPollKey(sandbox),
                sandbox.workspaces.map((row) => row.status),
              ]),
              sandboxRefreshDelay(
                sandbox,
                sandbox.workspaces.some(
                  (workspace) =>
                    workspace.status === 'preparing' || workspace.status === 'removing',
                ),
              ),
            ),
          );
        })
        .catch((error: unknown) => {
          if (abort.signal.aborted) return;
          const denied = error instanceof ApiError && [401, 403, 404, 410].includes(error.status);
          if (denied) views?.forgetSandbox(sandboxId);
          setStored((previous) => {
            const current = previous?.request.pollScope === pollScope ? previous.state : undefined;
            const cached =
              views?.client === detailReadSource(client) && views.viewerId === viewerId
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
          const retryAfter =
            error instanceof ApiError && [429, 503].includes(error.status)
              ? error.retryAfterMs
              : undefined;
          const wait = Math.max(POLL_ERROR_DELAY, retryAfter ?? 0);
          if (retryAfter !== undefined) floor.at = performance.now() + wait;
          if (!document.hidden) timer = setTimeout(poll, wait);
        })
        .finally(() => {
          inFlight = false;
        });
    };
    const visible = () => {
      clearTimeout(timer);
      if (document.hidden) return;
      delay.reset();
      poll();
    };
    document.addEventListener('visibilitychange', visible);
    pollNow.current = visible;
    poll();
    return () => {
      abort.abort();
      pollNow.current = undefined;
      clearTimeout(timer);
      document.removeEventListener('visibilitychange', visible);
    };
  }, [client, sandboxId, request, views, viewerId, pollScope]);
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
        <Button onClick={() => pollNow.current?.()}>Try again</Button>
      </section>
    );
  const { sandbox } = state;
  const needsConnections =
    viewerId !== undefined && sandbox.owner === viewerId && !connectionsReady;
  const lifecycleAction = sandboxLifecycleAction(sandbox, pollError);
  const lifecycleAllowed =
    lifecycleAction.kind === 'revive_sandbox' ? canResume : lifecycleAction.allowed;
  // An explicit attempt keeps the connection UI while the Paused page observes its outcome.
  const resumeOwnsConnections =
    canResume || (resumeCheck?.request === request && (sandbox.status === 'paused' || busy));
  const verifyConnectionsOnOpen = needsConnections && !stale && !resumeOwnsConnections;
  const lifecycleReason = permissions.pauseSandbox ? lifecycleAction.reason : undefined;
  const actionContext = { busy: submitting, lifecycleBusy, stale, needsConnections };
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
      <div ref={runtimePanel} tabIndex={-1}>
        <SandboxRuntimeUpdate
          client={client}
          sandboxId={sandboxId}
          ownerId={viewerId}
          revision={sandbox.revision}
          update={sandbox.runtimeUpdate}
          sandboxStatus={sandbox.status}
          technicalDetails="disclosure"
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
      </div>
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
        meta={<span>Shared environment for your Agents</span>}
        actions={
          <>
            {runtime !== undefined && sandboxHasParkedRuntimeUpdate(sandbox) ? (
              <Button
                size="sm"
                onClick={() => {
                  selectTab('overview');
                  requestAnimationFrame(() => runtimePanel.current?.focus());
                }}
              >
                Review runtime update
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
                disabled={busy || stale || !lifecycleAllowed}
                aria-describedby={
                  lifecycleAction.reason === undefined ? undefined : lifecycleReasonId
                }
                onClick={() => {
                  if (lifecycleAction.kind === 'pause_sandbox') {
                    onAction({ kind: 'pause_sandbox', sandboxId });
                  } else {
                    const controller = new AbortController();
                    setResumeCheck({ request, controller });
                    onAction({ kind: 'revive_sandbox', sandboxId }, controller.signal);
                  }
                }}
              >
                {lifecycleAction.label}
              </Button>
            ) : null}
            <MoreActions
              items={sandboxHeaderMenu(sandbox, permissions, actionContext, sharingAllowed)}
              onSelect={(id) => {
                if (id === 'new_agent') onAction({ kind: 'new_agent', sandboxId });
                else if (id === 'new_workspace') onAction({ kind: 'new_workspace', sandboxId });
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
      {verifyConnectionsOnOpen ? (
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
      {viewerId !== undefined && sandbox.owner === viewerId && client.inspectSandboxRecovery ? (
        <SandboxRetainedRecovery
          key={`${sandbox.id}:${viewerId}`}
          client={client}
          sandboxId={sandbox.id}
          ownerId={viewerId}
          revision={sandbox.revision}
          failed={sandbox.status === 'failed'}
          stale={stale}
          lifecycle={sandbox.lifecycle}
          disabled={busy || needsConnections}
          onAccepted={() => setRetry((value) => value + 1)}
        />
      ) : null}
      <SandboxSections
        key={`${sandbox.id}:${sandbox.owner ?? ''}:${JSON.stringify(sandbox.profile)}`}
        client={client}
        sandbox={sandbox}
        owner={viewerId !== undefined && sandbox.owner === viewerId ? viewerId : undefined}
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
        onAlwaysOnChange={
          viewerId !== undefined && sandbox.owner === viewerId
            ? async (alwaysOn) => {
                if (state.status !== 'ready') return;
                const updated = await client.setSandboxAlwaysOn(
                  sandboxId,
                  alwaysOn,
                  state.sandbox.revision,
                  { idempotencyKey: `always-on:${sandboxId}:${Date.now()}` },
                );
                setStored((previous) =>
                  previous?.request === request && previous.state.status === 'ready'
                    ? { ...previous, state: { ...previous.state, sandbox: updated } }
                    : previous,
                );
              }
            : undefined
        }
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
