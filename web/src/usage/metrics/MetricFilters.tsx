import type { ReactNode } from 'react';
import type { UsageMetricReport } from '@cma/client';
import { useMetricFilterDraft } from './useMetricFilterDraft';
import { Button } from '../../ui/Button';
import { Input } from '../../ui/Input';
import { Picker } from '../../ui/Picker';
import { ReportSelector } from './ReportSelector';
import {
  metricQuery,
  statusOptions,
  type MetricsClient,
  type Range,
  type Selection,
} from './model';

function localInput(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return '';
  return new Date(date.getTime() - date.getTimezoneOffset() * 60000).toISOString().slice(0, 16);
}
function utcValue(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? '' : date.toISOString();
}
export function MetricFilters({
  api,
  selection,
  admin,
  active,
  onApply,
  agentPicker,
  sandboxReport,
  selectionUnavailable = false,
}: {
  readonly api: MetricsClient;
  readonly selection: Selection;
  readonly admin: boolean;
  readonly active: boolean;
  readonly onApply: (value: Selection) => void;
  readonly agentPicker?: (draft: Selection, update: (value: Selection) => void) => ReactNode;
  readonly sandboxReport?: UsageMetricReport;
  readonly selectionUnavailable?: boolean;
}) {
  const { draft, error, changed, update, setError } = useMetricFilterDraft(selection);
  return (
    <form
      className="usage-metric-filters"
      data-admin={admin}
      aria-label="Usage filters"
      onSubmit={(event) => {
        event.preventDefault();
        try {
          metricQuery(draft, admin);
          setError('');
          onApply(draft);
        } catch (failure) {
          setError(failure instanceof Error ? failure.message : 'Invalid range.');
        }
      }}
    >
      <label className="usage-filter-field">
        <span>Time range</span>
        <Picker
          placement="usage-filter-control"
          label="Usage time range"
          value={draft.range}
          options={[
            { value: '1h', label: 'Last hour' },
            { value: '24h', label: 'Last 24 hours' },
            { value: '7d', label: 'Last 7 days' },
            { value: '30d', label: 'Last 30 days' },
            { value: 'custom', label: 'Custom (up to 30 days)' },
          ]}
          onChange={(range: Range) => update({ ...draft, range })}
        />
      </label>
      {draft.range === 'custom' && (
        <>
          <label className="usage-filter-field">
            <span>From (local time)</span>
            <Input
              placement="usage-filter-control"
              aria-label="Usage range from"
              type="datetime-local"
              required
              value={localInput(draft.from)}
              onChange={(event) => update({ ...draft, from: utcValue(event.target.value) })}
            />
          </label>
          <label className="usage-filter-field">
            <span>To (local time)</span>
            <Input
              placement="usage-filter-control"
              aria-label="Usage range to"
              type="datetime-local"
              required
              value={localInput(draft.to)}
              onChange={(event) => update({ ...draft, to: utcValue(event.target.value) })}
            />
          </label>
        </>
      )}
      {admin && (
        <div className="usage-filter-discovery">
          <ReportSelector
            api={api}
            selection={selection}
            admin
            active={active}
            kind="owner"
            value={draft.owner}
            onChange={(owner) => update({ ...draft, owner, sandbox: '', agent: '' })}
          />
        </div>
      )}
      {admin && (
        <label className="usage-filter-field">
          <span>Status</span>
          <Picker
            placement="usage-filter-control"
            label="Usage Sandbox status"
            value={draft.status}
            options={[{ value: '', label: 'Any status' }, ...statusOptions]}
            onChange={(status) => update({ ...draft, status })}
          />
        </label>
      )}
      {admin && (
        <label className="usage-filter-field">
          <span>Runtime version</span>
          <Input
            placement="usage-filter-control"
            aria-label="Usage runtime version"
            value={draft.runtime}
            maxLength={255}
            onChange={(event) => update({ ...draft, runtime: event.target.value })}
          />
        </label>
      )}
      <label className="usage-filter-field">
        <span>Sandbox name or ID starts with</span>
        <Input
          placement="usage-filter-control"
          type="search"
          value={draft.search}
          maxLength={200}
          onChange={(event) => update({ ...draft, search: event.target.value })}
        />
      </label>
      <div className="usage-filter-discovery">
        <ReportSelector
          api={api}
          selection={{ ...selection, owner: draft.owner }}
          admin={admin}
          active={active}
          kind="sandbox"
          value={draft.sandbox}
          selectedReport={sandboxReport}
          unavailable={selectionUnavailable && draft.sandbox === selection.sandbox}
          onChange={(sandbox) => update({ ...draft, sandbox, agent: '' })}
        />
      </div>
      {agentPicker && <div className="usage-filter-agent">{agentPicker(draft, update)}</div>}
      <div className="usage-filter-submit">
        <Button
          variant="primary"
          type="submit"
          placement="usage-filter-apply"
          aria-label="Apply usage filters"
        >
          Apply filters
        </Button>
      </div>
      {changed && (
        <p className="usage-filter-notice" role="status">
          Changes apply when you select Apply filters.
        </p>
      )}
      {error && <p role="alert">{error}</p>}
    </form>
  );
}
