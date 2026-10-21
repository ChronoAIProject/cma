import { Picker } from '../ui/Picker';
import { useCallback, useState } from 'react';
import type { ActivityAction, ActivityRecord } from '@chronoai/cma-sdk';
import { Badge } from '../ui/Badge';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { Table, TableCell } from '../ui/Table';
import { IconSearchOutline16 } from '../ui/icons';
import type { StatusTone } from '../vocabulary/status';
import type { OperatorClient } from './client';
import { FailureNotice, RecordControls, Timestamp, stateWord, useRecords } from './records';

const COLUMNS = [
  { header: 'Time (SGT)' },
  { header: 'Actor' },
  { header: 'Action' },
  { header: 'Target' },
  { header: 'Outcome' },
  { header: 'Code' },
];

// A refusal is the platform declining as designed; a failure is something broken.
const OUTCOMES = {
  succeeded: { label: 'Succeeded', tone: 'neutral' },
  refused: { label: 'Refused', tone: 'warning' },
  failed: { label: 'Failed', tone: 'error' },
} as const satisfies Record<
  ActivityRecord['outcome']['status'],
  { readonly label: string; readonly tone: StatusTone }
>;

function actionWord(action: ActivityAction): string {
  return action.charAt(0).toUpperCase() + action.slice(1);
}

const actions: readonly ActivityAction[] = [
  'authenticate',
  'create',
  'read',
  'list',
  'update',
  'delete',
  'configure',
];

export function Activity({ client }: { readonly client: OperatorClient }) {
  const [actor, setActor] = useState('');
  const [action, setAction] = useState<ActivityAction | ''>('');
  const [filters, setFilters] = useState({ actor, action });
  return (
    <section aria-label="Operator activity">
      <h2>Activity</h2>
      <form
        className="operator-filters"
        onSubmit={(event) => {
          event.preventDefault();
          setFilters({ actor, action });
        }}
      >
        <label className="operator-field">
          <span>Actor</span>
          <Input
            aria-label="Activity actor"
            value={actor}
            maxLength={255}
            onChange={(event) => setActor(event.target.value)}
          />
        </label>
        <label className="operator-field">
          <span>Action</span>
          <Picker
            label="Activity action"
            value={action}
            onChange={(choice) => setAction(choice as ActivityAction | '')}
            options={[
              { value: '', label: 'All actions' },
              ...actions.map((value) => ({ value, label: stateWord(value) })),
            ]}
          />
        </label>
        <Button type="submit" icon={<IconSearchOutline16 />}>
          Apply filters
        </Button>
      </form>
      <ActivityRecords key={JSON.stringify(filters)} client={client} filters={filters} />
    </section>
  );
}

function ActivityRecords({
  client,
  filters,
}: {
  readonly client: OperatorClient;
  readonly filters: { readonly actor: string; readonly action: ActivityAction | '' };
}) {
  const load = useCallback(
    (cursor: string | undefined, signal: AbortSignal) =>
      client.admin.listActivity({
        cursor,
        signal,
        limit: 50,
        actor: filters.actor || undefined,
        action: filters.action || undefined,
      }),
    [client, filters],
  );
  const records = useRecords(load);
  return (
    <>
      <RecordControls {...records} />
      <FailureNotice failure={records.error} />
      <Table
        label="Activity"
        placement="operator-table"
        columns={COLUMNS}
        loading={records.loading}
        loadingLabel="Loading records"
        empty={records.page === undefined ? undefined : 'No records match these filters.'}
      >
        {records.page?.items.map((row) => (
          <tr key={row.id}>
            <TableCell>
              <Timestamp value={row.at} />
            </TableCell>
            <TableCell>
              {row.actor === null ? 'Not signed in' : row.actor_name?.trim() || 'Name unavailable'}
            </TableCell>
            <TableCell>{actionWord(row.action)}</TableCell>
            <TableCell>
              {row.target === null ? null : row.target.kind === 'subject' ? (
                <>User {row.target_name?.trim() || 'Name unavailable'}</>
              ) : (
                <>
                  {row.target.kind} <code className="operator-id">{row.target.id}</code>
                </>
              )}
            </TableCell>
            <TableCell>
              <Badge tone={OUTCOMES[row.outcome.status].tone}>
                {OUTCOMES[row.outcome.status].label}
              </Badge>
            </TableCell>
            <TableCell mono>{'code' in row.outcome ? row.outcome.code : null}</TableCell>
          </tr>
        ))}
      </Table>
    </>
  );
}
