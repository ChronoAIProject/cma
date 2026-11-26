import { Picker } from '../ui/Picker';
import { useCallback, useEffect, useState } from 'react';
import type { ActivityAction, ActivitySummary } from '@cma/client';
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
  ActivitySummary['outcome']['status'],
  { readonly label: string; readonly tone: StatusTone }
>;

function actionWord(action: ActivityAction): string {
  return action.charAt(0).toUpperCase() + action.slice(1);
}

function actorLabel(row: ActivitySummary): string {
  if (row.actor === null) return 'No actor recorded';
  const name = row.actor_name?.trim();
  if (name) return name;
  return row.actor_reference ? `Unresolved actor · ${row.actor_reference}` : 'Unresolved actor';
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

type ActorOption = { readonly value: string; readonly label: string };

export function Activity({
  client,
  canSearchActors = false,
}: {
  readonly client: OperatorClient;
  readonly canSearchActors?: boolean;
}) {
  const [actor, setActor] = useState('');
  const [action, setAction] = useState<ActivityAction | 'audit' | ''>('audit');
  const [filters, setFilters] = useState({ actor, action });
  const [search, setSearch] = useState('');
  const [reply, setReply] = useState<{ text: string; options?: readonly ActorOption[] }>();
  const [selection, setSelection] = useState<ActorOption>();
  const [recent, setRecent] = useState<readonly ActorOption[]>([]);
  const text = search.trim();
  const bytes = new TextEncoder().encode(text).length;
  const searching = canSearchActors && bytes >= 2 && bytes <= 128;
  const result = searching && reply?.text === text ? reply : undefined;
  useEffect(() => {
    if (!searching) return;
    const controller = new AbortController();
    const timer = setTimeout(() => {
      void client.admin.searchActivityActors(text, { signal: controller.signal }).then(
        (people) => {
          if (controller.signal.aborted) return;
          setReply({
            text,
            options: people.map((person) => ({
              value: person.subject,
              label: person.display_name?.trim() || 'Name unavailable',
            })),
          });
        },
        () => {
          if (!controller.signal.aborted) setReply({ text });
        },
      );
    }, 250);
    return () => {
      clearTimeout(timer);
      controller.abort();
    };
  }, [client, searching, text]);
  const rememberActors = useCallback((items: readonly ActivitySummary[]) => {
    setRecent((previous) => {
      const known = new Map(previous.map((option) => [option.value, option]));
      for (const row of items) {
        if (row.actor === null || known.has(row.actor)) continue;
        known.set(row.actor, {
          value: row.actor,
          label: actorLabel(row),
        });
      }
      return [...known.values()].sort((a, b) => a.label.localeCompare(b.label));
    });
  }, []);
  const choices = new Map<string, ActorOption>([['', { value: '', label: 'All actors' }]]);
  for (const option of text ? (result?.options ?? []) : recent) choices.set(option.value, option);
  if (text && bytes <= 255 && !choices.has(text))
    choices.set(text, { value: text, label: `Use exact actor ID ${text}` });
  if (selection && !choices.has(selection.value)) choices.set(selection.value, selection);
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
          <span>{canSearchActors ? 'Find actor by name or ID' : 'Find actor by ID'}</span>
          <Input
            type="search"
            value={search}
            maxLength={255}
            onChange={(event) => {
              const next = event.target.value;
              if (next.trim() !== text) setReply(undefined);
              setSearch(next);
            }}
          />
          {searching && result === undefined ? <span role="status">Finding actors…</span> : null}
          {result && !result.options ? <span role="status">Actor search unavailable.</span> : null}
        </label>
        <label className="operator-field">
          <span>Actor</span>
          <Picker
            label="Activity actor"
            value={actor}
            onChange={(value) => {
              setActor(value);
              setSelection(choices.get(value));
            }}
            options={[...choices.values()]}
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
  const count = records.page?.items.length ?? 0;
  return (
    <>
      <RecordControls
        {...records}
        position={
          records.page && !records.loading ? (
            <span role="status">
              Page {records.pageNumber} ·{' '}
              {count === 0
                ? '0 records'
                : `Records ${records.offset + 1}–${records.offset + count}`}{' '}
              · {records.next ? 'More records available' : 'Last page'}
            </span>
          ) : undefined
        }
      />
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
            <TableCell>{actorLabel(row)}</TableCell>
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
