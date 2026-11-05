import { useState, type ReactNode } from 'react';
import { Button } from '../../ui/Button';
import { Input } from '../../ui/Input';
import { Picker } from '../../ui/Picker';
import { ReportSelector } from './ReportSelector';
import { metricQuery, statuses, type MetricsClient, type Range, type Selection } from './model';

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
}: {
  readonly api: MetricsClient;
  readonly selection: Selection;
  readonly admin: boolean;
  readonly active: boolean;
  readonly onApply: (value: Selection) => void;
  readonly agentPicker?: (draft: Selection, update: (value: Selection) => void) => ReactNode;
}) {
  const [stored, setStored] = useState({ selection, draft: selection });
  const [error, setError] = useState('');
  if (stored.selection !== selection) setStored({ selection, draft: selection });
  const draft = stored.selection === selection ? stored.draft : selection;
  const update = (value: Selection) => setStored({ selection, draft: value });
  return (
    <form
      className="usage-metric-filters"
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
      <label>
        <span>Time range</span>
        <Picker
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
          <label>
            <span>From (local time)</span>
            <Input
              aria-label="Usage range from"
              type="datetime-local"
              required
              value={localInput(draft.from)}
              onChange={(event) => update({ ...draft, from: utcValue(event.target.value) })}
            />
          </label>
          <label>
            <span>To (local time)</span>
            <Input
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
        <ReportSelector
          api={api}
          selection={selection}
          admin
          active={active}
          kind="owner"
          value={draft.owner}
          onChange={(owner) => update({ ...draft, owner, sandbox: '', agent: '' })}
        />
      )}
      {admin && (
        <label>
          <span>Status</span>
          <Picker
            label="Usage Sandbox status"
            value={draft.status}
            options={[
              { value: '', label: 'Any status' },
              ...statuses.map((value) => ({ value, label: value.replaceAll('_', ' ') })),
            ]}
            onChange={(status) => update({ ...draft, status })}
          />
        </label>
      )}
      {admin && (
        <label>
          <span>Runtime version</span>
          <Input
            aria-label="Usage runtime version"
            value={draft.runtime}
            maxLength={255}
            onChange={(event) => update({ ...draft, runtime: event.target.value })}
          />
        </label>
      )}
      <label>
        <span>Search name or ID</span>
        <Input
          type="search"
          aria-label="Usage Sandbox search"
          value={draft.search}
          maxLength={200}
          onChange={(event) => update({ ...draft, search: event.target.value })}
        />
      </label>
      <ReportSelector
        api={api}
        selection={{ ...selection, owner: draft.owner }}
        admin={admin}
        active={active}
        kind="sandbox"
        value={draft.sandbox}
        onChange={(sandbox) => update({ ...draft, sandbox, agent: '' })}
      />
      {agentPicker?.(draft, update)}
      <Button type="submit">Apply usage filters</Button>
      {error && <p role="alert">{error}</p>}
    </form>
  );
}
