import { useCallback, useMemo, useState } from 'react';
import { ProtocolError, type UsageMetricReport } from '@cma/client';
import { Button } from '../../ui/Button';
import { IconChevronDownOutline14 } from '../../ui/icons/IconChevronDownOutline14';
import { Input } from '../../ui/Input';
import { Picker } from '../../ui/Picker';
import { useUsageRead, useUsageReport } from './useReport';
import { MetricsNotice } from './MetricsNotice';
import type { MetricsClient, Selection } from './model';
import { resourceLabelKey, useSelectedResourceLabel } from './selectedResourceLabel';

/** Discovery is bounded and server-searched; no current table page defines the available choices. */
export function ReportSelector({
  api,
  selection,
  kind,
  value,
  onChange,
  active,
  admin,
  selectedReport,
  unavailable = false,
}: {
  readonly api: MetricsClient;
  readonly selection: Selection;
  readonly kind: 'owner' | 'sandbox';
  readonly value: string;
  readonly onChange: (value: string) => void;
  readonly active: boolean;
  readonly admin: boolean;
  readonly selectedReport?: UsageMetricReport;
  readonly unavailable?: boolean;
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
  const reportId = selectedReport?.report_id;
  const principal = selectedReport?.scope.principal;
  const resolve = useCallback(
    async (signal: AbortSignal) => {
      if (!reportId) throw new Error('No authorized report');
      const row = await api.sandbox(reportId, value, { signal });
      if (row.sandbox_id !== value || row.owner.subject !== principal)
        throw new ProtocolError('Sandbox selection did not match');
      return row.name.trim() || 'Unnamed Sandbox';
    },
    [api, reportId, value, principal],
  );
  const selected = useSelectedResourceLabel({
    key: resourceLabelKey('sandbox', value),
    load: resolve,
    active:
      active &&
      !admin &&
      kind === 'sandbox' &&
      !!value &&
      !unavailable &&
      selectedReport?.scope.admin === false &&
      selectedReport.status === 'ready' &&
      selectedReport.query.filters.sandbox_id === value &&
      Number(selectedReport.summary?.sandbox_count) > 0,
    scope: api,
  });
  const selectedName = !value
    ? all
    : kind === 'owner'
      ? value
      : unavailable || selected.error
        ? 'Selected Sandbox unavailable'
        : (selected.name ?? 'Loading Sandbox…');
  return (
    <div
      className="usage-metric-selector"
      onKeyDown={(event) => {
        if (!open || event.key !== 'Escape' || event.defaultPrevented) return;
        event.preventDefault();
        event.stopPropagation();
        setOpen(false);
        event.currentTarget.querySelector('button')?.focus({ preventScroll: true });
      }}
    >
      <span className="usage-filter-label">{label}</span>
      <Button
        variant="outline"
        placement="usage-discovery-control"
        aria-label={`${label}: ${selectedName}`}
        size="md"
        aria-expanded={open}
        onClick={() => setOpen((previous) => !previous)}
      >
        <span className="usage-discovery-value" title={selectedName}>
          {selectedName}
        </span>
        <IconChevronDownOutline14 />
      </Button>
      {kind === 'sandbox' && value && (
        <Button size="sm" onClick={() => onChange('')}>
          Clear Sandbox
        </Button>
      )}
      {kind === 'sandbox' && !unavailable && (
        <MetricsNotice
          failure={selected.error}
          onRetry={selected.retry}
          retryLabel="Retry Sandbox name"
        />
      )}
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
              selectedName={selectedName}
              active={active}
              onChange={(choice, name) => {
                if (kind === 'sandbox' && choice && name)
                  selected.remember(resourceLabelKey('sandbox', choice), name);
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
  selectedName,
}: {
  readonly api: MetricsClient;
  readonly reportId: string;
  readonly kind: 'owner' | 'sandbox';
  readonly search: string;
  readonly value: string;
  readonly active: boolean;
  readonly onChange: (value: string, name?: string) => void;
  readonly selectedName: string;
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
            name: owner.display_name || owner.subject,
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
          label: `${sandbox.name.trim() || 'Unnamed Sandbox'} (${sandbox.sandbox_id})`,
          name: sandbox.name.trim() || 'Unnamed Sandbox',
        })),
      };
    },
    [api, reportId, kind, search, cursor],
  );
  const page = useUsageRead(load, active, api);
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
                ? [{ value, label: selectedName }]
                : []),
            ]}
            onChange={(id) => {
              const option = page.data?.options.find((item) => item.value === id);
              onChange(id, option?.name);
            }}
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
