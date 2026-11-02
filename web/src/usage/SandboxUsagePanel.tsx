import { useEffect, useMemo, useState } from 'react';
import type { SandboxUsagePage, SandboxUsageSummary } from '@chronoai/cma-sdk';
import type { OperatorClient } from '../operator/client';
import { Alert } from '../ui/Alert';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { Table, TableCell } from '../ui/Table';
import { stateWord } from '../operator/records';
import { UsageHistory } from './UsageHistory';
import { uptime, usageTime } from './format';

const quantity = new Intl.NumberFormat('en', { maximumFractionDigits: 3 });
export function SandboxUsagePanel({
  client,
  admin = false,
}: {
  readonly client: OperatorClient['usage'];
  readonly admin?: boolean;
}) {
  const [ownerInput, setOwnerInput] = useState('');
  const [owner, setOwner] = useState('');
  const [cursor, setCursor] = useState<string>();
  const [revision, setRevision] = useState(0);
  const [selected, setSelected] = useState<string>();
  const request = useMemo(
    () => ({ client, admin, owner, cursor, revision }),
    [client, admin, owner, cursor, revision],
  );
  const [stored, setStored] = useState<{
    request: typeof request;
    summary?: SandboxUsageSummary;
    page?: SandboxUsagePage;
    error?: string;
  }>();
  useEffect(() => {
    const controller = new AbortController();
    const options = { signal: controller.signal, owner: owner || undefined, cursor, limit: 50 };
    void Promise.all([
      admin ? client.adminSummary(options) : client.summary(options),
      admin ? client.adminSandboxes(options) : client.sandboxes(options),
    ]).then(
      ([summary, page]) => {
        if (!controller.signal.aborted) setStored({ request, summary, page });
      },
      () => {
        if (!controller.signal.aborted)
          setStored({ request, error: 'Usage could not be loaded. Try refreshing.' });
      },
    );
    return () => controller.abort();
  }, [request, client, admin, owner, cursor]);
  const result = stored?.request === request ? stored : undefined;
  const summary = result?.summary;
  const page = result?.page;
  const loading = result === undefined;
  return (
    <section aria-label="Sandbox usage" className="operator-detail">
      <h2>Sandbox usage</h2>
      <p>
        Allocated CPU, RAM and disk over time. CPU and RAM accrue while a runtime is running;
        retained disk continues while paused.
      </p>
      <p>
        These totals describe allocated capacity. They do not measure CPU load, memory in use, or
        files stored.
      </p>
      <div className="operator-actions">
        <Button disabled={loading} onClick={() => setRevision((value) => value + 1)}>
          Refresh usage
        </Button>
        {summary ? <span>Updated {usageTime(summary.as_of)} SGT</span> : null}
      </div>
      {admin ? (
        <form
          className="operator-filters"
          onSubmit={(event) => {
            event.preventDefault();
            setOwner(ownerInput.trim());
            setCursor(undefined);
            setSelected(undefined);
          }}
        >
          <label className="operator-field">
            <span>Owner ID</span>
            <Input
              value={ownerInput}
              onChange={(event) => setOwnerInput(event.target.value)}
              placeholder="All users"
            />
          </label>
          <Button type="submit">Apply filter</Button>
        </form>
      ) : null}
      {result?.error ? (
        <Alert tone="error" title="Usage unavailable">
          {result.error}
        </Alert>
      ) : null}
      {summary ? (
        <>
          <dl>
            <dt>Sandboxes</dt>
            <dd>
              {summary.sandbox_count} ({summary.running_count} running)
            </dd>
            <dt>Total uptime</dt>
            <dd>{uptime(summary.totals.uptime_seconds)}</dd>
            <dt>CPU</dt>
            <dd>{quantity.format(summary.totals.cpu_core_hours)} core-hours</dd>
            <dt>RAM</dt>
            <dd>{quantity.format(summary.totals.memory_gib_hours)} GiB-hours</dd>
            <dt>Disk</dt>
            <dd>{quantity.format(summary.totals.disk_gib_hours)} GiB-hours</dd>
          </dl>
          {summary.incomplete_lifecycle_count > 0 ? (
            <Alert tone="info" title="Some usage history is unavailable">
              {summary.incomplete_lifecycle_count}{' '}
              {summary.incomplete_lifecycle_count === 1 ? 'sandbox has' : 'sandboxes have'} partial
              history. Totals include recorded intervals; usage before tracking or during gaps is
              unavailable.
            </Alert>
          ) : null}
        </>
      ) : null}
      <Table
        label="Usage by sandbox"
        loading={loading}
        empty="No sandbox usage has been recorded."
        columns={[
          { header: 'Sandbox' },
          ...(admin ? [{ header: 'Owner ID' }] : []),
          { header: 'Status' },
          { header: 'Uptime' },
          { header: 'CPU (core-hours)', align: 'end' },
          { header: 'RAM (GiB-hours)', align: 'end' },
          { header: 'Disk (GiB-hours)', align: 'end' },
          { header: 'Tracking since (SGT)' },
        ]}
      >
        {page?.items.map((row) => (
          <tr key={row.sandbox_id}>
            <TableCell>
              <Button size="sm" onClick={() => setSelected(row.sandbox_id)}>
                {row.name}
              </Button>
              <div>{row.sandbox_id}</div>
            </TableCell>
            {admin ? <TableCell mono>{row.owner}</TableCell> : null}
            <TableCell>{stateWord(row.status)}</TableCell>
            <TableCell>{uptime(row.totals.uptime_seconds)}</TableCell>
            <TableCell align="end">{quantity.format(row.totals.cpu_core_hours)}</TableCell>
            <TableCell align="end">{quantity.format(row.totals.memory_gib_hours)}</TableCell>
            <TableCell align="end">{quantity.format(row.totals.disk_gib_hours)}</TableCell>
            <TableCell>
              {usageTime(row.tracking_started_at)}
              {row.complete_lifecycle ? '' : ' (partial history)'}
            </TableCell>
          </tr>
        ))}
      </Table>
      <div className="operator-actions">
        {cursor ? <Button onClick={() => setCursor(undefined)}>First page</Button> : null}
        {page?.next ? (
          <Button onClick={() => setCursor(page.next ?? undefined)}>Next page</Button>
        ) : null}
      </div>
      {selected ? (
        <UsageHistory
          key={`${selected}:${String(revision)}`}
          client={client}
          id={selected}
          onClose={() => setSelected(undefined)}
        />
      ) : null}
    </section>
  );
}
