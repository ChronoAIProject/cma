import { Picker } from '../ui/Picker';
import { useCallback, useEffect, useState } from 'react';
import type { ActivityAction, ActivitySummary } from '@cma/client';
import { Badge } from '../ui/Badge';
import { Button } from '../ui/Button';
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
  ActivitySummary['outcome']['status'],
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
  'gateway_record_write',
  'gateway_record_read',
  'gateway_record_delete',
];

export function Activity({ client }: { readonly client: OperatorClient }) {
  const [actor, setActor] = useState('');
  const [action, setAction] = useState<ActivityAction | 'audit' | ''>('audit');
  const [filters, setFilters] = useState({ actor, action });
  // The public operator API currently returns canonical actor names alongside
  // activity records, but has no separate actor catalogue endpoint. Accumulate
  // actors as pages arrive so the filter is still a stable selector instead of
  // asking an operator to type an opaque subject id.
  const [actorOptions, setActorOptions] = useState<readonly { value: string; label: string }[]>([]);
  const rememberActors = useCallback((items: readonly ActivitySummary[]) => {
    setActorOptions((previous) => {
      const known = new Map(previous.map((option) => [option.value, option]));
      for (const row of items) {
        if (row.actor === null || known.has(row.actor)) continue;
        known.set(row.actor, {
          value: row.actor,
          label: row.actor_name?.trim() || 'Name unavailable',
        });
      }
      return [...known.values()].sort((a, b) => a.label.localeCompare(b.label));
    });
  }, []);
  return (
    <section aria-label="Operator activity">
      <h2>Activity</h2>
      <p>Newest recorded first.</p>
      <form
        className="operator-filters"
        onSubmit={(event) => {
          event.preventDefault();
          setFilters({ actor, action });
        }}
      >
        <label className="operator-field">
          <span>Actor</span>
          <Picker
            label="Activity actor"
            value={actor}
            onChange={setActor}
            options={[{ value: '', label: 'All actors' }, ...actorOptions]}
          />
        </label>
        <label className="operator-field">
          <span>Action</span>
          <Picker
            label="Activity action"
            value={action}
            onChange={(choice) => setAction(choice as ActivityAction | 'audit' | '')}
            options={[
              { value: 'audit', label: 'Audit events' },
              { value: '', label: 'All actions' },
              ...actions.map((value) => ({ value, label: stateWord(value) })),
            ]}
          />
        </label>
        <Button type="submit" icon={<IconSearchOutline16 />}>
          Apply filters
        </Button>
      </form>
      <ActivitySummarys
        key={JSON.stringify(filters)}
        client={client}
        filters={filters}
        onItemsLoaded={rememberActors}
      />
    </section>
  );
}

function ActivitySummarys({
  client,
  filters,
  onItemsLoaded,
}: {
  readonly client: OperatorClient;
  readonly filters: { readonly actor: string; readonly action: ActivityAction | 'audit' | '' };
  readonly onItemsLoaded: (items: readonly ActivitySummary[]) => void;
}) {
  const load = useCallback(
    (cursor: string | undefined, signal: AbortSignal) =>
      client.admin.listActivitySummary({
        cursor,
        signal,
        limit: 50,
        actor: filters.actor || undefined,
        action: filters.action === 'audit' ? undefined : filters.action || undefined,
        excludeSuccessfulReads: filters.action === 'audit' || undefined,
      }),
    [client, filters],
  );
  const records = useRecords(load, { retainOnError: false });
  useEffect(() => {
    if (records.page !== undefined) onItemsLoaded(records.page.items);
  }, [records.page, onItemsLoaded]);
  return (
    <>
      <RecordControls {...records} />
      <FailureNotice failure={records.error} />
      <Table
        label="Activity"
        placement="operator-table"
        columns={COLUMNS}
        loading={records.loading}
        loadingLabel={records.page === undefined ? 'Loading records' : 'Refreshing records'}
        empty={records.page === undefined ? undefined : 'No records match these filters.'}
      >
        {/* The API cursor orders each page by recorded time, then record ID. */}
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
