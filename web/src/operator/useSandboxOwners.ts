import { useEffect, useMemo, useState } from 'react';
import type { OperatorClient } from './client';

interface OwnerOption {
  readonly value: string;
  readonly label: string;
}

type OwnerState =
  | { readonly status: 'loading' }
  | { readonly status: 'ready'; readonly options: readonly OwnerOption[] }
  | { readonly status: 'error' };

/** One unfiltered scan per mounted client, independent of table filters and paging. */
export function useSandboxOwners(client: OperatorClient) {
  const [revision, setRevision] = useState(0);
  const request = useMemo(() => ({ client, revision }), [client, revision]);
  const [stored, setStored] = useState<{ request: typeof request; state: OwnerState }>();
  useEffect(() => {
    const controller = new AbortController();
    const timeout = setTimeout(() => {
      controller.abort();
      setStored({ request, state: { status: 'error' } });
    }, 15_000);
    const load = async () => {
      const owners = new Map<string, OwnerOption>();
      const cursors = new Set<string>();
      let cursor: string | undefined;
      for (let pageNumber = 0; pageNumber < 100; pageNumber += 1) {
        const page = await client.admin.listSandboxes({
          cursor,
          limit: 200,
          signal: controller.signal,
        });
        if (controller.signal.aborted) return;
        for (const row of page.items) {
          const label = row.owner_name?.trim();
          if (label || !owners.has(row.owner))
            owners.set(row.owner, { value: row.owner, label: label || 'Name unavailable' });
        }
        if (page.next === undefined || page.next === null) {
          setStored({
            request,
            state: {
              status: 'ready',
              options: [...owners.values()].sort((a, b) => a.label.localeCompare(b.label)),
            },
          });
          return;
        }
        if (page.next === '' || cursors.has(page.next))
          throw new Error('Owner inventory did not advance.');
        cursors.add(page.next);
        cursor = page.next;
      }
      throw new Error('Owner inventory exceeded its page limit.');
    };
    void load()
      .catch(() => {
        if (!controller.signal.aborted) setStored({ request, state: { status: 'error' } });
      })
      .finally(() => clearTimeout(timeout));
    return () => {
      controller.abort();
      clearTimeout(timeout);
    };
  }, [client, request]);
  return {
    state: stored?.request === request ? stored.state : ({ status: 'loading' } as const),
    retry: () => setRevision((value) => value + 1),
  };
}
