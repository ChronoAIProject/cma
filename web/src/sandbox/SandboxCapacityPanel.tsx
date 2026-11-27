import type { SandboxStatus } from '@cma/client';
import { useEffect, useState } from 'react';
import type { CmaWebClient } from '../api/client';
import type { SandboxSummary } from '../api/types';
import { routePath } from '../routing/routes';
import { DocsLink } from '../services/DocsLink';
import { allSandboxes } from '../shell/navigation';
import { Button } from '../ui/Button';
import { Modal } from '../ui/Modal';
import { Table, TableCell } from '../ui/Table';
import { statusLabel } from '../vocabulary/status';
import { IconWarningOutline16 } from '../ui/icons';
import { sandboxLifecycleLabel } from './lifecycle';
import type { useSandboxCapacity } from './useSandboxCapacity';
import './SandboxCapacityPanel.css';

export function SandboxCapacityPanel({
  client,
  capacity,
  presentation,
  onEnvironments,
}: {
  readonly client: CmaWebClient;
  readonly capacity: ReturnType<typeof useSandboxCapacity>;
  readonly presentation?: 'home';
  readonly onEnvironments?: () => void;
}) {
  const [manage, setManage] = useState(false);
  const value = capacity.value;
  if (presentation === 'home') {
    if (!capacity.error && value && value.available > 1)
      return manage ? (
        <EnvironmentInventory client={client} onClose={() => setManage(false)} />
      ) : null;
    const near = !capacity.error && value?.available === 1;
    return (
      <section
        className="sandbox-capacity-home"
        data-state={capacity.error ? 'unknown' : near ? 'near' : value ? 'full' : 'loading'}
        aria-label="Sandbox capacity"
      >
        {capacity.error ? (
          <div className="sandbox-capacity-home__message" role="alert">
            <IconWarningOutline16 />
            <div>
              <strong>Capacity could not be checked</strong>
              <p>
                {capacity.error}. Your draft is kept.{' '}
                {value
                  ? 'The last observation is stale; available slots are unconfirmed.'
                  : 'Available slots are unconfirmed.'}
              </p>
            </div>
          </div>
        ) : !value ? (
          <p role="status">Checking Sandbox capacity…</p>
        ) : near ? (
          <p className="sandbox-capacity-home__near">
            <span>1 slot left</span>
          </p>
        ) : (
          <div className="sandbox-capacity-home__message">
            <IconWarningOutline16 />
            <div>
              <strong>Environment limit reached</strong>
              {(value.counts_by_status.retiring ?? 0) > 0 ? (
                <p>Deletions still using slots: {value.counts_by_status.retiring}</p>
              ) : null}
            </div>
          </div>
        )}
        {!near ? (
          <div className="sandbox-capacity__actions">
            <Button variant="outline" size="sm" onClick={onEnvironments ?? (() => setManage(true))}>
              Environments
            </Button>
            {capacity.error ? (
              <Button
                variant="outline"
                size="sm"
                disabled={capacity.loading}
                onClick={() => void capacity.refresh()}
              >
                Check again
              </Button>
            ) : (
              <Button size="sm" disabled={capacity.loading} onClick={() => void capacity.refresh()}>
                Refresh Sandbox capacity
              </Button>
            )}
          </div>
        ) : null}
        {manage ? <EnvironmentInventory client={client} onClose={() => setManage(false)} /> : null}
      </section>
    );
  }
  return (
    <section className="card sandbox-capacity" aria-label="Sandbox capacity">
      {capacity.error ? (
        <p className="notice__detail" role="alert">
          {capacity.error}. {value ? 'The last observation is stale.' : ''}
        </p>
      ) : !value ? (
        <p role="status">Checking Sandbox capacity…</p>
      ) : null}
      {value ? (
        <>
          <p className="notice__title">
            {value.live}/{value.limit} Sandbox environments used
          </p>
          <p className="notice__detail">
            {Object.entries(value.counts_by_status)
              .map(([status, count]) => `${statusLabel(status as SandboxStatus)}: ${count}`)
              .join(' · ')}
          </p>
          {value.available === 0 ? (
            <p className="notice__detail">
              All {value.limit} Sandbox environment slots are in use. Your Agent list and
              environment count are different. Manage environments to see what occupies these slots.
            </p>
          ) : null}
          {value.counts_by_status.retiring ? (
            <p className="notice__detail">
              {value.counts_by_status.retiring}{' '}
              {value.counts_by_status.retiring === 1
                ? 'environment is being deleted and still uses a slot.'
                : 'environments are being deleted and still use slots.'}{' '}
              Slots become available after runtime and credential cleanup is confirmed.
            </p>
          ) : null}
          {(value.retiring_released ?? 0) > 0 ? (
            <p className="notice__detail">
              Deletions that have released their slots: {value.retiring_released}
            </p>
          ) : null}
        </>
      ) : null}
      <div className="sandbox-capacity__actions">
        <Button variant="outline" size="sm" onClick={() => setManage(true)}>
          Manage environments
        </Button>
        <Button size="sm" disabled={capacity.loading} onClick={() => void capacity.refresh()}>
          Refresh Sandbox capacity
        </Button>
        <DocsLink view="environments-and-capacity">Help with environments and capacity</DocsLink>
      </div>
      {manage ? <EnvironmentInventory client={client} onClose={() => setManage(false)} /> : null}
    </section>
  );
}

function EnvironmentInventory({
  client,
  onClose,
}: {
  readonly client: CmaWebClient;
  readonly onClose: () => void;
}) {
  const [rows, setRows] = useState<readonly SandboxSummary[]>();
  const [error, setError] = useState<string>();
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    const abort = new AbortController();
    void allSandboxes(client, abort.signal).then(
      (items) => {
        if (!abort.signal.aborted) {
          setRows(items);
          setError(undefined);
        }
      },
      () => {
        if (!abort.signal.aborted)
          setError('Environments could not be refreshed. Any previous list is stale.');
      },
    );
    return () => abort.abort();
  }, [client, revision]);
  return (
    <Modal
      open
      title="Manage environments"
      onClose={onClose}
      footer={
        <>
          <Button variant="outline" onClick={() => setRevision((value) => value + 1)}>
            Refresh environments
          </Button>
          <Button onClick={onClose}>Done</Button>
        </>
      }
      description="These are your Sandbox environments, including those with no Agents. Open an environment to review its contents and available actions, or create an Agent in an existing eligible environment."
    >
      {error ? (
        <p role="alert">{error}</p>
      ) : !rows ? (
        <p role="status">Loading environments…</p>
      ) : null}
      {rows ? (
        <Table
          label="Your Sandbox environments"
          columns={[{ header: 'Environment' }, { header: 'Status' }]}
          empty="No non-retired Sandbox environments."
        >
          {rows.map((sandbox) => (
            <tr key={sandbox.id}>
              <TableCell>
                <a href={routePath({ kind: 'sandbox', sandboxId: sandbox.id })}>{sandbox.name}</a>
              </TableCell>
              <TableCell>
                {sandbox.status === 'retiring'
                  ? sandbox.lifecycle?.result?.ownerSlotReleased
                    ? 'Deleting · Sandbox slot released'
                    : 'Deleting · Sandbox slot in use'
                  : sandboxLifecycleLabel(sandbox)}
              </TableCell>
            </tr>
          ))}
        </Table>
      ) : null}
    </Modal>
  );
}
