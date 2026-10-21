import { Picker } from '../ui/Picker';
import { Presence } from '../ui/Presence';
import { useCallback, useRef, useState } from 'react';
import type { AdminSandboxSummary, SandboxStatus } from '@chronoai/cma-sdk';
import { Alert } from '../ui/Alert';
import { StatusBadge } from '../ui/Badge';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { RiskConfirmation } from '../ui/RiskConfirmation';
import { Table, TableActions, TableCell } from '../ui/Table';
import { IconArchiveOutline20, IconInspectOutline12, IconSearchOutline16 } from '../ui/icons';
import type { OperatorClient } from './client';
import { FailureNotice, RecordControls, Timestamp, stateWord, useRecords } from './records';
import { operatorFailure, type OperatorFailure } from './failure';
import { SandboxDetail } from './SandboxDetail';

const COLUMNS = [
  { header: 'Sandbox' },
  { header: 'Owner' },
  { header: 'Status' },
  { header: 'Runtime' },
  { header: 'Agents', align: 'end' },
  { header: 'Last activity (SGT)' },
] as const;

const statuses: readonly SandboxStatus[] = [
  'provisioning',
  'installing',
  'active',
  'pausing',
  'paused',
  'reviving',
  'credential_rotating',
  'rotating',
  'failed',
  'retiring',
  'retired',
];

export function Sandboxes({
  client,
  canControl,
}: {
  readonly client: OperatorClient;
  readonly canControl: boolean;
}) {
  const [owner, setOwner] = useState('');
  const [status, setStatus] = useState<SandboxStatus | ''>('');
  const [version, setVersion] = useState('');
  const [filters, setFilters] = useState({ owner, status, version });
  return (
    <section aria-label="Sandbox inventory">
      <h2>Sandboxes</h2>
      {/* Said once for the page: every row would otherwise repeat the same sentence. */}
      {canControl ? null : (
        <div className="operator-notices">
          <Alert tone="info" title="Cross-owner control is required.">
            You can review these Sandboxes, but Retire stays unavailable.
          </Alert>
        </div>
      )}
      <form
        className="operator-filters"
        onSubmit={(event) => {
          event.preventDefault();
          setFilters({ owner, status, version });
        }}
      >
        <label className="operator-field">
          <span>Owner</span>
          <Input
            aria-label="Sandbox owner"
            value={owner}
            onChange={(event) => setOwner(event.target.value)}
            maxLength={255}
          />
        </label>
        <label className="operator-field">
          <span>Status</span>
          <Picker
            label="Sandbox status"
            value={status}
            onChange={(choice) => setStatus(choice as SandboxStatus | '')}
            options={[
              { value: '', label: 'All statuses' },
              ...statuses.map((value) => ({ value, label: stateWord(value) })),
            ]}
          />
        </label>
        <label className="operator-field">
          <span>Runtime version</span>
          <Input
            aria-label="Runtime version"
            value={version}
            onChange={(event) => setVersion(event.target.value)}
            maxLength={255}
          />
        </label>
        <Button type="submit" icon={<IconSearchOutline16 />}>
          Apply filters
        </Button>
      </form>
      <SandboxRecords
        key={JSON.stringify(filters)}
        client={client}
        canControl={canControl}
        filters={filters}
      />
    </section>
  );
}

function SandboxRecords({
  client,
  canControl,
  filters,
}: {
  readonly client: OperatorClient;
  readonly canControl: boolean;
  readonly filters: {
    readonly owner: string;
    readonly status: SandboxStatus | '';
    readonly version: string;
  };
}) {
  const load = useCallback(
    (cursor: string | undefined, signal: AbortSignal) =>
      client.admin.listSandboxes({
        cursor,
        signal,
        limit: 50,
        owner: filters.owner || undefined,
        status: filters.status || undefined,
        runtime_version: filters.version || undefined,
      }),
    [client, filters],
  );
  const records = useRecords(load);
  const [detail, setDetail] = useState<string>();
  const [target, setTarget] = useState<AdminSandboxSummary>();
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<OperatorFailure>();
  const [uncertain, setUncertain] = useState(false);
  const submitting = useRef(false);
  const retire = async () => {
    if (target === undefined || !canControl || submitting.current) return;
    submitting.current = true;
    setBusy(true);
    setFailure(undefined);
    try {
      if (uncertain) {
        const current = await client.admin.readSandbox(target.id);
        setTarget({ ...target, status: current.sandbox.status });
        setUncertain(false);
        records.refresh();
        if (current.sandbox.status === 'retiring' || current.sandbox.status === 'retired')
          setTarget(undefined);
      } else {
        await client.admin.retireSandbox(target.id);
        setTarget(undefined);
        records.refresh();
      }
    } catch (error) {
      const problem = operatorFailure(error);
      setFailure(problem);
      setUncertain((previous) => previous || problem.uncertain);
    } finally {
      submitting.current = false;
      setBusy(false);
    }
  };
  return (
    <>
      <RecordControls {...records} />
      <FailureNotice failure={records.error} />
      <Table
        label="Sandboxes"
        placement="operator-table"
        columns={COLUMNS}
        actions="Actions"
        loading={records.loading}
        loadingLabel="Loading records"
        empty={records.page === undefined ? undefined : 'No records match these filters.'}
      >
        {records.page?.items.map((row) => (
          <tr key={row.id}>
            <TableCell>
              <span className="operator-name">{row.name}</span>
              <code className="operator-id">{row.id}</code>
            </TableCell>
            <TableCell>{row.owner_name?.trim() || 'Name unavailable'}</TableCell>
            <TableCell>
              <StatusBadge status={row.status} />
            </TableCell>
            <TableCell mono>{row.runtime_version ?? 'not bound'}</TableCell>
            <TableCell align="end">{row.agent_count}</TableCell>
            <TableCell>
              <Timestamp value={row.last_activity_at} />
            </TableCell>
            <TableActions>
              <Button
                size="sm"
                icon={<IconInspectOutline12 />}
                onClick={() => setDetail(row.id)}
                aria-label={`Details for ${row.name}`}
              >
                Details
              </Button>
              {/* Without the capability the page says so once; a Deleting or
                  Deleted badge is this row's own reason. */}
              <Button
                size="sm"
                variant="danger"
                icon={<IconArchiveOutline20 />}
                disabled={!canControl || row.status === 'retired' || row.status === 'retiring'}
                onClick={() => {
                  setFailure(undefined);
                  setUncertain(false);
                  setTarget(row);
                }}
              >
                Retire
              </Button>
            </TableActions>
          </tr>
        ))}
      </Table>
      {detail === undefined ? null : (
        <SandboxDetail client={client} id={detail} onClose={() => setDetail(undefined)} />
      )}
      <Presence>
        {target === undefined || !canControl ? null : (
          <RiskConfirmation
            open
            title={`Retire ${target.name}`}
            description={`Retiring Sandbox ${target.id} stops its Agents and releases its runtime. It cannot be resumed. The operator action is audited.`}
            acknowledgeLabel="I understand this Sandbox cannot be resumed."
            confirmLabel={uncertain ? 'Check current state' : 'Retire Sandbox'}
            busy={busy}
            dismissible={!busy}
            failure={
              failure === undefined
                ? undefined
                : `${failure.message}${failure.code === undefined ? '' : ` (${failure.code})`}`
            }
            status={
              uncertain
                ? 'The previous outcome is unknown. Only a read will be sent next.'
                : busy
                  ? 'Submitting'
                  : undefined
            }
            onCancel={() => setTarget(undefined)}
            onConfirm={() => void retire()}
          />
        )}
      </Presence>
    </>
  );
}
