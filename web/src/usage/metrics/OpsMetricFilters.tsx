import { useState } from 'react';
import { Button } from '../../ui/Button';
import { Input } from '../../ui/Input';
import { Picker } from '../../ui/Picker';
import { FilterOptionPicker } from './FilterOptionPicker';
import {
  defaultSelection,
  metricQuery,
  statuses,
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

export function OpsMetricFilters({
  api,
  selection,
  active,
  onApply,
}: {
  readonly api: MetricsClient;
  readonly selection: Selection;
  readonly active: boolean;
  readonly onApply: (value: Selection) => void;
}) {
  const [stored, setStored] = useState({ selection, draft: selection });
  const [error, setError] = useState('');
  if (stored.selection !== selection) setStored({ selection, draft: selection });
  const draft = stored.selection === selection ? stored.draft : selection;
  const update = (next: Partial<Selection>) =>
    setStored({ selection, draft: { ...draft, ...next } });
  return (
    <form
      className="usage-metric-filters"
      data-admin="true"
      data-dropdowns="true"
      aria-label="Usage filters"
      onSubmit={(event) => {
        event.preventDefault();
        try {
          const next = { ...draft, search: '', agent: '' };
          metricQuery(next, true);
          setError('');
          onApply(next);
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
          searchable
          options={[
            { value: '1h', label: 'Last hour' },
            { value: '24h', label: 'Last 24 hours' },
            { value: '7d', label: 'Last 7 days' },
            { value: '30d', label: 'Last 30 days' },
            { value: 'custom', label: 'Custom (up to 30 days)' },
          ]}
          onChange={(range: Range) => update({ range })}
        />
      </label>
      <FilterOptionPicker
        api={api}
        kind="owner"
        value={draft.owner}
        active={active}
        onChange={(owner) => update({ owner, sandbox: '', agent: '' })}
      />
      <label className="usage-filter-field">
        <span>Status</span>
        <Picker
          placement="usage-filter-control"
          label="Usage Sandbox status"
          value={draft.status}
          searchable
          options={[
            { value: '', label: 'Any status' },
            ...statuses.map((value) => ({ value, label: value.replaceAll('_', ' ') })),
          ]}
          onChange={(status) => update({ status })}
        />
      </label>
      <FilterOptionPicker
        api={api}
        kind="runtime"
        value={draft.runtime}
        active={active}
        onChange={(runtime) => update({ runtime })}
      />
      <FilterOptionPicker
        key={draft.owner}
        api={api}
        kind="sandbox"
        owner={draft.owner || undefined}
        value={draft.sandbox}
        active={active}
        onChange={(sandbox) => update({ sandbox, agent: '', search: '' })}
      />
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
              onChange={(event) => update({ from: utcValue(event.target.value) })}
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
              onChange={(event) => update({ to: utcValue(event.target.value) })}
            />
          </label>
        </>
      )}
      {selection.search && (
        <p role="status">
          This saved report has a legacy search. Choose a Sandbox and apply filters to replace it.
        </p>
      )}
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
      <Button
        onClick={() => {
          setError('');
          onApply({ ...defaultSelection, sort: selection.sort, order: selection.order });
        }}
      >
        Clear filters
      </Button>
      {error && <p role="alert">{error}</p>}
    </form>
  );
}
