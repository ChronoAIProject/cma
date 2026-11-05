import { useCallback, useMemo, useState } from 'react';
import { Button } from '../../ui/Button';
import { Input } from '../../ui/Input';
import { Picker } from '../../ui/Picker';
import { useUsageRead, useUsageReport } from './useReport';
import { MetricsNotice } from './MetricsNotice';
import type { MetricsClient, Selection } from './model';

/** Discovery is bounded and server-searched; no current table page defines the available choices. */
export function ReportSelector({
  api,
  selection,
  kind,
  value,
  onChange,
  active,
  admin,
}: {
  readonly api: MetricsClient;
  readonly selection: Selection;
  readonly kind: 'owner' | 'sandbox';
  readonly value: string;
  readonly onChange: (value: string) => void;
  readonly active: boolean;
  readonly admin: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState('');
  const [appliedSearch, setAppliedSearch] = useState('');
  const discovery = useMemo(
    () => ({
      ...selection,
      owner: kind === 'owner' ? '' : selection.owner,
      status: '' as const,
      runtime: '',
      sandbox: '',
      agent: '',
      search: kind === 'sandbox' ? appliedSearch : '',
    }),
    [selection, kind, appliedSearch],
  );
  const report = useUsageReport(api, discovery, admin, active && open);
  const label = kind === 'owner' ? 'Owner' : 'Sandbox';
  const all = kind === 'owner' ? 'Everyone' : admin ? 'All Sandboxes' : 'All my Sandboxes';
  return (
    <div className="usage-metric-selector">
      <Button size="sm" aria-expanded={open} onClick={() => setOpen((previous) => !previous)}>
        {label}: {value || all}
      </Button>
      {open && (
        <div>
          <Input
            aria-label={`Search ${label.toLowerCase()} choices`}
            value={search}
            onChange={(event) => setSearch(event.target.value)}
            maxLength={200}
            placeholder={kind === 'owner' ? 'Owner subject prefix' : 'Sandbox name or ID'}
          />
          <Button size="sm" onClick={() => setAppliedSearch(search.trim())}>
            Search {label.toLowerCase()} choices
          </Button>
          <Button
            size="sm"
            onClick={() => {
              onChange('');
              setOpen(false);
            }}
          >
            {all}
          </Button>
          <MetricsNotice failure={report.error} onRetry={report.refresh} />
          {report.busy && <p role="status">Loading {label.toLowerCase()} choices…</p>}
          {!report.busy && report.report && (
            <ChoicePage
              key={report.report.report_id + appliedSearch}
              api={api}
              reportId={report.report.report_id}
              kind={kind}
              search={appliedSearch}
              value={value}
              active={active}
              onChange={(choice) => {
                onChange(choice);
                setOpen(false);
              }}
            />
          )}
        </div>
      )}
    </div>
  );
}

function ChoicePage({
  api,
  reportId,
  kind,
  search,
  value,
  active,
  onChange,
}: {
  readonly api: MetricsClient;
  readonly reportId: string;
  readonly kind: 'owner' | 'sandbox';
  readonly search: string;
  readonly value: string;
  readonly active: boolean;
  readonly onChange: (value: string) => void;
}) {
  const [cursors, setCursors] = useState<readonly (string | undefined)[]>([undefined]);
  const cursor = cursors[cursors.length - 1];
  const load = useCallback(
    async (signal: AbortSignal) => {
      if (kind === 'owner') {
        const page = await api.owners(reportId, { search, cursor, limit: 50, signal });
        return {
          next: page.next,
          options: page.items.map((owner) => ({
            value: owner.subject,
            label: owner.display_name ? `${owner.display_name} (${owner.subject})` : owner.subject,
          })),
        };
      }
      const page = await api.sandboxes(reportId, {
        cursor,
        limit: 50,
        sort: 'name',
        order: 'asc',
        signal,
      });
      return {
        next: page.next,
        options: page.items.map((sandbox) => ({
          value: sandbox.sandbox_id,
          label: `${sandbox.name} (${sandbox.sandbox_id})`,
        })),
      };
    },
    [api, reportId, kind, search, cursor],
  );
  const page = useUsageRead(load, active);
  return (
    <>
      <MetricsNotice failure={page.error} onRetry={page.retry} />
      {page.loading && <p role="status">Loading choices…</p>}
      {page.data && (
        <>
          <Picker
            label={`Choose ${kind}`}
            value={value}
            options={[
              { value: '', label: 'Select a choice' },
              ...page.data.options,
              ...(value && !page.data.options.some((option) => option.value === value)
                ? [{ value, label: value }]
                : []),
            ]}
            onChange={onChange}
          />
          {page.data.options.length === 0 && <p>No matching {kind} choices.</p>}
          <Button
            size="sm"
            disabled={cursors.length === 1}
            onClick={() => setCursors((previous) => previous.slice(0, -1))}
          >
            Previous choices
          </Button>
          <Button
            size="sm"
            disabled={!page.data.next}
            onClick={() => {
              const next = page.data?.next;
              if (next && !cursors.includes(next)) setCursors((previous) => [...previous, next]);
            }}
          >
            Next choices
          </Button>
        </>
      )}
    </>
  );
}
