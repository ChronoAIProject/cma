import { Picker } from '../ui/Picker';
import { useCallback, useState } from 'react';
import type { ActivityAction } from '@chronoai/cma-sdk';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { IconSearchOutline16 } from '../ui/icons';
import type { OperatorClient } from './client';
import {
  FailureNotice,
  RecordControls,
  Records,
  Timestamp,
  stateWord,
  useRecords,
} from './records';

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
      <Records loading={records.loading} empty={records.page?.items.length === 0}>
        {records.page?.items.map((row) => (
          <article className="operator-record" key={row.id}>
            <Timestamp value={row.at} />
            <div>
              <p>
                {stateWord(row.action)}: {row.outcome.status}
              </p>
              <p className="operator-meta">
                <span>
                  {row.actor === null ? (
                    'Not signed in'
                  ) : (
                    <>Actor {row.actor_name?.trim() || 'Name unavailable'}</>
                  )}
                </span>
                {row.target === null ? null : (
                  <span>
                    {row.target.kind === 'subject' ? (
                      <>User {row.target_name?.trim() || 'Name unavailable'}</>
                    ) : (
                      <>
                        {row.target.kind} <code>{row.target.id}</code>
                      </>
                    )}
                  </span>
                )}
              </p>
              {'code' in row.outcome ? <code>{row.outcome.code}</code> : null}
            </div>
            <code>{row.id}</code>
          </article>
        ))}
      </Records>
    </>
  );
}
