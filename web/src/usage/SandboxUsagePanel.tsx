import { useEffect, useMemo, useState } from 'react';
import type { SandboxUsagePage, SandboxUsageSummary } from '@chronoai/cma-sdk';
import type { OperatorClient } from '../operator/client';
import { Alert } from '../ui/Alert';
import { Button } from '../ui/Button';
import { Table, TableCell } from '../ui/Table';
import { stateWord } from '../operator/records';
import { UsageHistory } from './UsageHistory';
import { uptime, usageTime } from './format';

const quantity = new Intl.NumberFormat('en', { maximumFractionDigits: 3 });
/** Legacy owner allocation view, retained until the account Usage popup lands. */
export function SandboxUsagePanel({ client }: { readonly client: OperatorClient['usage'] }) {
  const [cursor, setCursor] = useState<string>();
  const [revision, setRevision] = useState(0);
  const [selected, setSelected] = useState<string>();
  const request = useMemo(() => ({ client, cursor, revision }), [client, cursor, revision]);
  const [stored, setStored] = useState<{
    request: typeof request;
    summary?: SandboxUsageSummary;
    page?: SandboxUsagePage;
    error?: string;
  }>();
  useEffect(() => {
    const controller = new AbortController();
    const failed = () =>
      setStored((previous) => ({
        ...(previous?.request.client === client ? previous : {}),
        request,
        error: 'Usage could not be loaded. Try refreshing.',
      }));
    const timeout = setTimeout(() => {
      controller.abort();
      failed();
    }, 30000);
    const options = { signal: controller.signal, cursor, limit: 50 };
    void Promise.all([client.summary(options), client.sandboxes(options)])
      .then(
        ([summary, page]) => {
          if (!controller.signal.aborted) setStored({ request, summary, page });
        },
        () => {
          if (!controller.signal.aborted) failed();
        },
      )
      .finally(() => clearTimeout(timeout));
    return () => {
      controller.abort();
      clearTimeout(timeout);
    };
  }, [request, client, cursor]);
  const result = stored?.request.client === client ? stored : undefined;
  const loading = result?.request !== request;
  const summary = result?.summary;
  const rows = result?.page?.items;
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
        <Button
          disabled={loading}
          onClick={() => {
            setRevision((value) => value + 1);
            setCursor(undefined);
          }}
        >
          Refresh usage
        </Button>
        {summary ? <span>Updated {usageTime(summary.as_of)} SGT</span> : null}
      </div>
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
        empty={
          result?.error && !rows
            ? 'Usage is unavailable.'
            : 'No sandbox usage matches these filters.'
        }
        columns={[
          { header: 'Sandbox' },
          { header: 'Status' },
          { header: 'Uptime' },
          { header: 'CPU (core-hours)', align: 'end' },
          { header: 'RAM (GiB-hours)', align: 'end' },
          { header: 'Disk (GiB-hours)', align: 'end' },
          { header: 'Tracking since (SGT)' },
        ]}
      >
        {rows?.map((row) => (
          <tr key={row.sandbox_id}>
            <TableCell>
              <Button size="sm" onClick={() => setSelected(row.sandbox_id)}>
                {row.name}
              </Button>
              <div>{row.sandbox_id}</div>
            </TableCell>
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
        {result?.page?.next ? (
          <Button onClick={() => setCursor(result.page?.next ?? undefined)}>Next page</Button>
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
