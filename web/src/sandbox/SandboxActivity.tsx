import { useEffect, useState } from 'react';
import { ApiError } from '@cma/client';
import type { CmaWebClient } from '../api/client';
import { activityEntry, activityLimit, type ActivityEntry } from './activity';
import { LiveActivity } from './LiveActivity';

interface Observation {
  readonly id: string;
  readonly entries: readonly ActivityEntry[];
  readonly syncedAt?: string;
  readonly stale: boolean;
  readonly loading: boolean;
}

export function SandboxActivity({
  client,
  sandboxId,
  settled = false,
}: {
  readonly client: CmaWebClient;
  readonly sandboxId: string;
  readonly settled?: boolean;
}) {
  const [observation, setObservation] = useState<Observation>();
  useEffect(() => {
    const abort = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let reading = false;
    let cursor = 0;
    let entries: ActivityEntry[] = [];
    let syncedAt: string | undefined;
    let finished = false;
    const poll = async () => {
      if (abort.signal.aborted || document.hidden || reading || finished) return;
      clearTimeout(timer);
      reading = true;
      let delay = 1500;
      try {
        let caughtUp = false;
        // Bound each catch-up batch as well as retained display state.
        for (let pages = 0; pages < 4; pages += 1) {
          const page = await client.readSandboxEvents(sandboxId, { cursor, signal: abort.signal });
          abort.signal.throwIfAborted();
          let nextCursor = cursor;
          const nextEntries: ActivityEntry[] = [];
          for (const event of page) {
            if (
              event.sandboxId !== sandboxId ||
              !Number.isSafeInteger(event.seq) ||
              event.seq <= nextCursor
            )
              throw new Error('Invalid activity sequence');
            nextCursor = event.seq;
            const entry = activityEntry(event, 'sandbox');
            if (entry !== undefined) nextEntries.push(entry);
          }
          cursor = nextCursor;
          entries = [...entries, ...nextEntries].slice(-activityLimit);
          if (page.length === 0) {
            caughtUp = true;
            break;
          }
        }
        if (caughtUp) syncedAt = new Date().toISOString();
        setObservation({ id: sandboxId, entries, syncedAt, stale: false, loading: !caughtUp });
        finished = settled && caughtUp;
      } catch (error) {
        if (abort.signal.aborted) return;
        if (error instanceof ApiError && [401, 403, 404].includes(error.status)) {
          entries = [];
          cursor = 0;
          syncedAt = undefined;
        }
        setObservation({ id: sandboxId, entries, syncedAt, stale: true, loading: false });
        delay = 5000;
      } finally {
        reading = false;
        if (!abort.signal.aborted && !finished) timer = setTimeout(() => void poll(), delay);
      }
    };
    const visible = () => void poll();
    document.addEventListener('visibilitychange', visible);
    void poll();
    return () => {
      abort.abort();
      clearTimeout(timer);
      document.removeEventListener('visibilitychange', visible);
    };
  }, [client, sandboxId, settled]);
  const current = observation?.id === sandboxId ? observation : undefined;
  return (
    <LiveActivity
      entries={current?.entries ?? []}
      syncedAt={current?.syncedAt}
      stale={current?.stale}
      loading={current?.loading ?? true}
    />
  );
}
