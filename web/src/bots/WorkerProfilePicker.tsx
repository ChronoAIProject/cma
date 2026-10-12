import { useEffect, useState } from 'react';
import type { EventWorkerProfileRecord, EventWorkerProfiles } from '@chronoai/cma-sdk';
import { Picker } from '../ui/Picker';
import { Button } from '../ui/Button';
import { problem } from './model';

export function WorkerProfilePicker({
  client,
  value,
  onChange,
}: {
  readonly client: Pick<EventWorkerProfiles, 'list'>;
  readonly value?: EventWorkerProfileRecord;
  readonly onChange: (value: EventWorkerProfileRecord | undefined) => void;
}) {
  const [rows, setRows] = useState<readonly EventWorkerProfileRecord[]>([]);
  const [cursor, setCursor] = useState<string>();
  const [next, setNext] = useState<string>();
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string>();
  const [reload, setReload] = useState(0);
  useEffect(() => {
    const abort = new AbortController();
    void client
      .list({ after: cursor, signal: abort.signal })
      .then((page) => {
        if (abort.signal.aborted) return;
        if (page.next_after === cursor && cursor !== undefined)
          throw new Error('Profile page did not advance.');
        setRows((current) =>
          cursor === undefined
            ? page.items
            : [
                ...current,
                ...page.items.filter((row) => !current.some((item) => item.id === row.id)),
              ],
        );
        setNext(page.next_after ?? undefined);
        setLoading(false);
        setError(undefined);
      })
      .catch((failure: unknown) => {
        if (!abort.signal.aborted) {
          setError(problem(failure));
          setLoading(false);
        }
      });
    return () => abort.abort();
  }, [client, cursor, reload]);
  return (
    <div className="profile-field">
      <Picker
        label="Event Worker Profile"
        value={value?.id ?? ''}
        disabled={loading}
        options={[
          { value: '', label: loading ? 'Loading worker profiles' : 'Choose a worker profile' },
          ...rows.map((row) => ({
            value: row.id,
            label: `${row.source.name} · Revision ${row.revision}`,
          })),
        ]}
        onChange={(id) => onChange(rows.find((row) => row.id === id))}
      />
      {value ? (
        <p>
          {value.profile.model.model} · {value.profile.permission.action_mode}. Includes the saved
          Sandbox and Workspace configuration.
        </p>
      ) : null}
      {error ? (
        <div role="alert">
          <p>{error}</p>
          <Button
            onClick={() => {
              setLoading(true);
              setReload((n) => n + 1);
            }}
          >
            Try again
          </Button>
        </div>
      ) : null}
      {!loading && !error && rows.length === 0 ? (
        <p>Create an Event Worker Profile in Profiles → Event workers first.</p>
      ) : null}
      {next === undefined ? null : (
        <Button
          disabled={loading}
          onClick={() => {
            setLoading(true);
            setCursor(next);
          }}
        >
          More profiles
        </Button>
      )}
    </div>
  );
}
