import { useMemo } from 'react';
import type { EffectiveAccess, OrganizationChoice } from '@cma/client';
import type { ResourceViewsClient } from './client';
import { useAuthorizedRead } from './useAuthorizedRead';
import { summarizeAccess, type AccessSummary } from './accessSummary';

const empty: ReadonlyMap<string, AccessSummary> = new Map();

/** Bounded current metadata for visible resources, without any ancestry reads. */
export function useAccessSummaries(
  client: ResourceViewsClient,
  kind: 'agent' | 'sandbox' | 'workspace',
  ids: readonly string[],
  scope = '',
): ReadonlyMap<string, AccessSummary> {
  const identity = JSON.stringify({ scope, ids: [...new Set(ids)].sort() });
  const read = useMemo(
    () => async (signal: AbortSignal) => {
      const targets = (JSON.parse(identity) as { ids: string[] }).ids;
      const access = new Map<string, EffectiveAccess>();
      const deadline = performance.now() + 12_000;
      let next = 0;
      let expires = Date.now() + 55_000;
      const worker = async () => {
        while (next < targets.length) {
          signal.throwIfAborted();
          const remaining = Math.floor(deadline - performance.now());
          if (remaining <= 0) return;
          const id = targets[next++];
          if (id === undefined) return;
          try {
            const value = await client.resourceAccess.effective(kind, id, {
              signal,
              timeoutMs: Math.min(3_000, remaining),
            });
            signal.throwIfAborted();
            const expiration =
              value.revalidate_at == null ? expires : Date.parse(value.revalidate_at);
            if (
              !value.capabilities.includes('view_details') ||
              !Number.isFinite(expiration) ||
              expiration <= Date.now()
            )
              continue;
            expires = Math.min(expires, expiration);
            access.set(id, value);
          } catch {
            signal.throwIfAborted();
          }
        }
      };
      await Promise.all(Array.from({ length: Math.min(4, targets.length) }, worker));
      let organizations: readonly OrganizationChoice[] = [];
      const remaining = Math.floor(deadline - performance.now());
      if (
        remaining > 0 &&
        [...access.values()].some((value) => value.owner?.kind === 'organization')
      ) {
        try {
          organizations = (
            await client.resourceAccess.organizations({
              signal,
              timeoutMs: Math.min(3_000, remaining),
            })
          ).items;
        } catch {
          signal.throwIfAborted();
        }
      }
      return {
        value: new Map(
          [...access].map(([id, value]) => [id, summarizeAccess(value, organizations)]),
        ),
        revalidateAt: new Date(expires).toISOString(),
      };
    },
    [client, kind, identity],
  );
  const current = useAuthorizedRead(read);
  return current.state.status === 'ready' ? current.state.value : empty;
}
