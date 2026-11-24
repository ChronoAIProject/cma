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
  parked = false,
}: {
  readonly client: CmaWebClient;
  readonly sandboxId: string;
  readonly settled?: boolean;
  readonly parked?: boolean;
}) {
  const [observation, setObservation] = useState<Observation>();
  useEffect(() => {
    const abort = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let reading = false;
    let retryAt = 0;
    let cursor = 0;
    let entries: ActivityEntry[] = [];
    let syncedAt: string | undefined;
    let finished = false;
    const poll = async () => {
      if (abort.signal.aborted || document.hidden || reading || finished) return;
      clearTimeout(timer);
      if (Date.now() < retryAt) {
        timer = setTimeout(() => void poll(), retryAt - Date.now());
        return;
      }
      reading = true;
      let delay = parked ? 5000 : 1500;
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
        retryAt = 0;
      } catch (error) {
        if (abort.signal.aborted) return;
        if (error instanceof ApiError && [401, 403, 404].includes(error.status)) {
          entries = [];
          cursor = 0;
          syncedAt = undefined;
        }
        setObservation({ id: sandboxId, entries, syncedAt, stale: true, loading: false });
        delay = Math.max(5000, error instanceof ApiError ? (error.retryAfterMs ?? 0) : 0);
        retryAt = Date.now() + delay;
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
  }, [client, sandboxId, settled, parked]);
  const current = observation?.id === sandboxId ? observation : undefined;
  // Stable sorting keeps sequence order for events recorded at the same time.
  const displayedEntries = current?.entries
    .slice()
    .sort((a, b) => Date.parse(a.at) - Date.parse(b.at));
  return (
    <LiveActivity
      entries={displayedEntries ?? []}
      syncedAt={current?.syncedAt}
      stale={current?.stale}
      loading={current?.loading ?? true}
    />
  );
}
