import { useEffect, useMemo, useState } from 'react';
import type { SandboxUsagePage, SandboxUsageSummary, SandboxUsageView } from '@chronoai/cma-sdk';
import type { OperatorClient } from '../operator/client';
import { Alert } from '../ui/Alert';
import { Button } from '../ui/Button';
import { UsageFilters, type UsageOwners } from './UsageFilters';
import {
  loadUsageInventory,
  selectUsage,
  summarizeUsage,
  type UsageFilters as Filters,
  type UsageSort,
  type UsageSortKey,
} from './inventory';
import { Table, TableCell } from '../ui/Table';
import { stateWord } from '../operator/records';
import { UsageHistory } from './UsageHistory';
import { uptime, usageTime } from './format';

const quantity = new Intl.NumberFormat('en', { maximumFractionDigits: 3 });
export function SandboxUsagePanel({
  client,
  admin = false,
  owners,
}: {
  readonly client: OperatorClient['usage'];
  readonly admin?: boolean;
  readonly owners?: UsageOwners;
}) {
  const [filters, setFilters] = useState<Filters>({ owner: '', status: '', sandbox: '' });
  const [sort, setSort] = useState<UsageSort>({ key: 'sandbox_id', direction: 'ascending' });
  const [pageIndex, setPageIndex] = useState(0);
  const [cursor, setCursor] = useState<string>();
  const [revision, setRevision] = useState(0);
  const [selected, setSelected] = useState<string>();
  const request = useMemo(
    () => ({ client, admin, cursor, revision }),
    [client, admin, cursor, revision],
  );
  const [stored, setStored] = useState<{
    request: typeof request;
    summary?: SandboxUsageSummary;
    page?: SandboxUsagePage;
    rows?: SandboxUsageView[];
    error?: string;
  }>();
  useEffect(() => {
    const controller = new AbortController();
    const timeout = setTimeout(() => {
      controller.abort();
      setStored((previous) => ({
        ...(previous?.request.client === client && previous.request.admin === admin
          ? previous
          : {}),
        request,
        error: 'The complete usage inventory could not be loaded. Try refreshing.',
      }));
    }, 30_000);
    const options = { signal: controller.signal, cursor, limit: 50 };
    const load = async () => {
      if (admin) {
        const [summary, rows] = await Promise.all([
          client.adminSummary(options),
          loadUsageInventory(client, controller.signal),
        ]);
        return { summary, rows };
      }
      const [summary, page] = await Promise.all([
        client.summary(options),
        client.sandboxes(options),
      ]);
      return { summary, page };
    };
    void load()
      .then(
        (value) => {
          if (!controller.signal.aborted) setStored({ request, ...value });
        },
        () => {
          if (!controller.signal.aborted)
            setStored((previous) => ({
              ...(previous?.request.client === client && previous.request.admin === admin
                ? previous
                : {}),
              request,
              error: 'Usage could not be loaded completely. Try refreshing.',
            }));
        },
      )
      .finally(() => clearTimeout(timeout));
    return () => {
      controller.abort();
      clearTimeout(timeout);
    };
  }, [request, client, admin, cursor]);
  const result =
    stored?.request.client === client && stored.request.admin === admin ? stored : undefined;
  const loading = result?.request !== request;
  const ownerNames = useMemo(
    () =>
      new Map(
        owners?.state.status === 'ready'
          ? owners.state.options.map((option) => [option.value, option.label])
          : [],
      ),
    [owners],
  );
  const filtered = useMemo(
    () =>
      result?.rows === undefined ? undefined : selectUsage(result.rows, filters, sort, ownerNames),
    [result, filters, sort, ownerNames],
  );
  const summary = filtered
    ? summarizeUsage(filtered, result?.summary?.as_of ?? 0)
    : result?.summary;
  const lastPage = Math.max(0, Math.ceil((filtered?.length ?? 0) / 50) - 1);
  const currentPage = Math.min(pageIndex, lastPage);
  const rows = admin
    ? filtered?.slice(currentPage * 50, (currentPage + 1) * 50)
    : result?.page?.items;
  const column = (header: string, key: UsageSortKey, align?: 'end') => ({
    header,
    align,
    ...(admin
      ? {
          sort: sort.key === key ? sort.direction : undefined,
          onSort: () => {
            setSort({
              key,
              direction:
                sort.key === key && sort.direction === 'ascending' ? 'descending' : 'ascending',
            });
            setPageIndex(0);
          },
        }
      : {}),
  });
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
            setPageIndex(0);
          }}
        >
          Refresh usage
        </Button>
        {summary ? <span>Updated {usageTime(summary.as_of)} SGT</span> : null}
      </div>
      {admin && owners ? (
        <UsageFilters
          owners={owners}
          rows={result?.rows ?? []}
          onApply={(value) => {
            setFilters(value);
            setPageIndex(0);
            setSelected(undefined);
          }}
        />
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
        empty={
          result?.error && !rows
            ? 'Usage is unavailable.'
            : 'No sandbox usage matches these filters.'
        }
        columns={[
          column('Sandbox', 'name'),
          ...(admin ? [column('Sandbox ID', 'sandbox_id'), column('User', 'owner')] : []),
          column('Status', 'status'),
          column('Uptime', 'uptime_seconds'),
          column('CPU (core-hours)', 'cpu_millis_seconds', 'end'),
          column('RAM (GiB-hours)', 'memory_mib_seconds', 'end'),
          column('Disk (GiB-hours)', 'disk_gib_seconds', 'end'),
          column('Tracking since (SGT)', 'tracking_started_at'),
        ]}
      >
        {rows?.map((row) => (
          <tr key={row.sandbox_id}>
            <TableCell>
              <Button size="sm" onClick={() => setSelected(row.sandbox_id)}>
                {row.name}
              </Button>
              {admin ? null : <div>{row.sandbox_id}</div>}
            </TableCell>
            {admin ? (
              <>
                <TableCell mono>{row.sandbox_id}</TableCell>
                <TableCell>
                  {ownerNames.get(row.owner) ?? `Name unavailable (${row.owner})`}
                </TableCell>
              </>
            ) : null}
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
        {admin && filtered ? (
          <>
            <span>
              {filtered?.length ?? 0} matching Sandboxes · Page {currentPage + 1} of {lastPage + 1}
            </span>
            <Button
              disabled={currentPage === 0 || loading}
              onClick={() => setPageIndex(currentPage - 1)}
            >
              Previous page
            </Button>
            <Button
              disabled={currentPage === lastPage || loading}
              onClick={() => setPageIndex(currentPage + 1)}
            >
              Next page
            </Button>
          </>
        ) : (
          <>
            {cursor ? <Button onClick={() => setCursor(undefined)}>First page</Button> : null}
            {result?.page?.next ? (
              <Button onClick={() => setCursor(result.page?.next ?? undefined)}>Next page</Button>
            ) : null}
          </>
        )}
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
