import { useId, useState } from 'react';
import type { SandboxUsageView } from '@chronoai/cma-sdk';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { Picker } from '../ui/Picker';
import { stateWord } from '../operator/records';
import type { useSandboxOwners } from '../operator/useSandboxOwners';
import type { UsageFilters as Filters } from './inventory';

export type UsageOwners = ReturnType<typeof useSandboxOwners>;
const statuses = [
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

export function UsageFilters({
  owners,
  rows,
  onApply,
}: {
  readonly owners: UsageOwners;
  readonly rows: readonly SandboxUsageView[];
  readonly onApply: (filters: Filters) => void;
}) {
  const [draft, setDraft] = useState<Filters>({ owner: '', status: '', sandbox: '' });
  const hint = useId();
  const options = new Map(
    (owners.state.status === 'ready' ? owners.state.options : []).map((owner) => [
      owner.value,
      owner,
    ]),
  );
  for (const row of rows) {
    if (!options.has(row.owner))
      options.set(row.owner, {
        value: row.owner,
        label: `Name unavailable (${row.owner})`,
      });
  }
  const labels = new Map<string, number>();
  for (const option of options.values())
    labels.set(option.label, (labels.get(option.label) ?? 0) + 1);
  const choices = [...options.values()]
    .map((option) => ({
      ...option,
      label:
        (labels.get(option.label) ?? 0) > 1 ? `${option.label} (${option.value})` : option.label,
    }))
    .sort((a, b) => a.label.localeCompare(b.label));
  return (
    <form
      className="operator-filters"
      onSubmit={(event) => {
        event.preventDefault();
        onApply({ ...draft, sandbox: draft.sandbox.trim() });
      }}
    >
      <label className="operator-field">
        <span>User</span>
        <Picker
          label="Usage user"
          value={draft.owner}
          onChange={(owner) => setDraft({ ...draft, owner })}
          options={[{ value: '', label: 'All users' }, ...choices]}
          disabled={choices.length === 0 && owners.state.status !== 'ready'}
          describedBy={owners.state.status === 'ready' ? undefined : hint}
        />
        {owners.state.status === 'loading' ? (
          <span id={hint} role="status">
            Loading user names…
          </span>
        ) : owners.state.status === 'error' ? (
          <span id={hint} role="alert">
            User names could not be loaded.
          </span>
        ) : null}
      </label>
      {owners.state.status === 'error' ? (
        <Button size="sm" onClick={owners.retry}>
          Retry user names
        </Button>
      ) : null}
      <label className="operator-field">
        <span>Sandbox status</span>
        <Picker
          label="Usage sandbox status"
          value={draft.status}
          onChange={(status) => setDraft({ ...draft, status })}
          options={[
            { value: '', label: 'All statuses' },
            ...[...new Set([...statuses, ...rows.map((row) => row.status)])].map((value) => ({
              value,
              label: stateWord(value),
            })),
          ]}
        />
      </label>
      <label className="operator-field">
        <span>Sandbox ID</span>
        <Input
          type="search"
          value={draft.sandbox}
          placeholder="Search by Sandbox ID"
          maxLength={256}
          onChange={(event) => setDraft({ ...draft, sandbox: event.target.value })}
        />
      </label>
      <Button type="submit">Apply filters</Button>
    </form>
  );
}
