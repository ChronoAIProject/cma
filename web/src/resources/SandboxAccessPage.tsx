import type { ApiError } from '@cma/client';
import { ResourceAccessDetails } from './ResourceAccessSummary';
import { SkeletonRows } from '../ui/Skeleton';
import { useEffect, useMemo, useState, type ReactNode } from 'react';
import { useOwnerViews } from './retainedOwnerViews';
import type { ResourceViewsClient } from './client';
import { routePath, type AppRoute } from '../routing/routes';
import { PageHeader } from '../ui/PageHeader';
import { StatusBadge } from '../ui/Badge';
import { useDocumentTitle } from '../ui/useDocumentTitle';
import { sandboxStatusPresentation } from '../sandbox/presentation';
import { loadSandbox } from './loadHierarchy';
import { useAuthorizedRead } from './useAuthorizedRead';
import { WorkspaceBranch } from './WorkspaceContents';
import { Button } from '../ui/Button';
import { resourceCount } from '../vocabulary/count';
import { useSandboxControl } from './useSandboxControl';
import { SandboxControls } from './SandboxControls';
import './SharedHierarchy.css';
import '../sandbox/SandboxPage.css';

export function SandboxAccessPage({
  client,
  id,
  personal,
  onNavigate,
  commandScope,
}: {
  readonly client?: ResourceViewsClient;
  readonly commandScope?: object;
  readonly id: string;
  readonly personal: ReactNode | ((accessUncertain: boolean) => ReactNode);
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const views = useOwnerViews();
  const retained =
    views !== undefined && views.client.resourceViews === client
      ? views.sandboxes.get(id)
      : undefined;
  const [knownPersonal, setKnownPersonal] = useState<{
    readonly client: ResourceViewsClient;
    readonly id: string;
    readonly views: typeof views;
  }>();
  const read = useMemo(
    () =>
      client === undefined
        ? undefined
        : async (signal: AbortSignal) => {
            const result = await loadSandbox(client, id, signal);
            signal.throwIfAborted();
            if (
              result.value.detail.access === 'shared' ||
              ['retiring', 'retired'].includes(result.value.detail.status)
            )
              views?.forgetSandbox(id);
            setKnownPersonal(
              result.value.detail.access === 'shared' ? undefined : { client, id, views },
            );
            return result;
          },
    [client, id, views],
  );
  const retryPolicy = useMemo(
    () => ({
      scope: read ?? {},
      applies: (error: ApiError) => error.status === 503 && error.retryAfterMs !== undefined,
    }),
    [read],
  );
  const current = useAuthorizedRead(read, false, retryPolicy);
  const transient = current.state.status === 'error' && current.state.transient === true;
  useEffect(() => {
    if (!transient) return;
    window.addEventListener('online', current.reload);
    return () => window.removeEventListener('online', current.reload);
  }, [transient, current.reload]);
  useEffect(() => {
    if (current.state.status === 'error') views?.forgetSandbox(id);
  }, [current.state, views, id]);
  const shared =
    current.state.status === 'ready' && current.state.value.detail.access === 'shared'
      ? current.state.value
      : undefined;
  const localCommandScope = useMemo(() => ({ client }), [client]);
  const control = useSandboxControl(
    client,
    id,
    shared && {
      revision: shared.detail.revision,
      access: shared.access,
    },
    current.reload,
    commandScope ?? localCommandScope,
  );
  // The personal page names its own tab; only a shared reader's is named here.
  useDocumentTitle(shared?.detail.name);
  if (knownPersonal !== undefined && current.state.status === 'error') setKnownPersonal(undefined);
  if (
    client === undefined ||
    (knownPersonal?.client === client &&
      knownPersonal?.id === id &&
      knownPersonal?.views === views &&
      current.state.status !== 'error') ||
    (retained !== undefined && current.state.status === 'loading')
  )
    return typeof personal === 'function'
      ? personal(client !== undefined && current.state.status !== 'ready')
      : personal;
  if (current.state.status === 'loading')
    return (
      <section className="console__empty">
        <h1>Opening Sandbox</h1>
        <SkeletonRows rows={3} label="Opening Sandbox" />
      </section>
    );
  if (current.state.status === 'error')
    return (
      <section className="console__empty" role="alert">
        <h1>Sandbox unavailable</h1>
        <p>{current.state.message}</p>
        {transient ? (
          <p>
            The current environment status could not be checked. Retry to read its status and access
            again.
          </p>
        ) : null}
        <Button onClick={current.reload}>
          {transient ? 'Retry environment check' : 'Check Sandbox access'}
        </Button>
      </section>
    );
  const sandbox = current.state.value.detail;
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
        badge={<StatusBadge status={sandboxStatusPresentation(sandbox).status} />}
        description={
          <>
            <ResourceAccessDetails client={client} access={current.state.value.access} />
            <p>
              {resourceCount(sandbox.workspace_count, 'Workspace')} ·{' '}
              {resourceCount(sandbox.agent_count, 'Agent')}
            </p>
          </>
        }
        actions={<Button onClick={current.reload}>Refresh Sandbox access</Button>}
      />
      <SandboxControls
        control={control}
        access={current.state.value.access}
        status={sandbox.status}
        revision={sandbox.revision}
      />
      <section className="sandbox-page__band">
        <h2>Workspaces</h2>
        {sandbox.workspaces.length === 0 ? (
          <p>No authorized Workspaces in this Sandbox.</p>
        ) : (
          <ul className="shared-hierarchy__workspaces">
            {sandbox.workspaces.map((workspace) => (
              <WorkspaceBranch
                key={workspace.id}
                client={client}
                id={workspace.id}
                sandboxId={id}
                name={workspace.name}
                onNavigate={onNavigate}
                onAccessChanged={current.reload}
              />
            ))}
          </ul>
        )}
      </section>
    </section>
  );
}
