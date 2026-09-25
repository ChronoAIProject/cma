import { ResourceAccessDetails } from './ResourceAccessSummary';
import { useMemo, useState, type ReactNode } from 'react';
import type { ResourceViewsClient } from './client';
import type { AppRoute } from '../routing/routes';
import { loadSandbox } from './loadHierarchy';
import { useAuthorizedRead } from './useAuthorizedRead';
import { WorkspaceBranch } from './WorkspaceContents';
import { Button } from '../ui/Button';
import { statusLabel } from '../vocabulary/status';
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
  readonly personal: ReactNode;
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const [knownPersonal, setKnownPersonal] = useState<{
    readonly client: ResourceViewsClient;
    readonly id: string;
  }>();
  const read = useMemo(
    () =>
      client === undefined
        ? undefined
        : async (signal: AbortSignal) => {
            const result = await loadSandbox(client, id, signal);
            signal.throwIfAborted();
            setKnownPersonal(result.value.detail.access === 'shared' ? undefined : { client, id });
            return result;
          },
    [client, id],
  );
  const current = useAuthorizedRead(read);
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
  if (knownPersonal !== undefined && current.state.status === 'error') setKnownPersonal(undefined);
  if (
    client === undefined ||
    (knownPersonal?.client === client &&
      knownPersonal?.id === id &&
      current.state.status !== 'error')
  )
    return personal;
  if (current.state.status === 'loading')
    return (
      <section className="console__empty" role="status">
        <h1>Opening Sandbox</h1>
      </section>
    );
  if (current.state.status === 'error')
    return (
      <section className="console__empty" role="alert">
        <h1>Sandbox unavailable</h1>
        <p>{current.state.message}</p>
        <Button onClick={current.reload}>Check Sandbox access</Button>
      </section>
    );
  const sandbox = current.state.value.detail;
  return (
    <section className="sandbox-page">
      <header className="sandbox-page__header">
        <div>
          <h1>{sandbox.name}</h1>
          <ResourceAccessDetails client={client} access={current.state.value.access} />
          <p>
            {statusLabel(sandbox.status)} · {resourceCount(sandbox.workspace_count, 'Workspace')} ·{' '}
            {resourceCount(sandbox.agent_count, 'Agent')}
          </p>
        </div>
        <div className="sandbox-page__actions">
          <Button onClick={current.reload}>Refresh Sandbox access</Button>
        </div>
      </header>
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
