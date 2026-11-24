import { ApiError, ProtocolError } from '@cma/client';
import { useEffect, useState } from 'react';
import type { SkillCatalogClient, SkillCatalogEntry } from '../profiles/client';
import { rateLimitDelay } from './transport/readRetry';

interface CatalogState {
  readonly source?: SkillCatalogClient;
  readonly active: boolean;
  readonly query: string;
  readonly cursor?: string;
  readonly revision: number;
  readonly entries: readonly SkillCatalogEntry[];
  readonly next?: string;
  readonly seen: readonly string[];
  readonly phase: 'loading' | 'ready' | 'failed';
  readonly loaded: boolean;
  readonly failure?: unknown;
  readonly retryAt: number;
  readonly notBefore: number;
}

const initial = (
  source: SkillCatalogClient | undefined,
  active: boolean,
  query = '',
  retryAt = 0,
): CatalogState => ({
  source,
  active,
  query,
  revision: 0,
  entries: [],
  seen: [],
  phase: 'loading',
  loaded: false,
  retryAt,
  notBefore: retryAt,
});

/** Catalog reads have no access to installation commands or their retained intent. */
export function useSkillCatalog(source: SkillCatalogClient | undefined, active: boolean) {
  const [state, setState] = useState(() => initial(source, active));
  if (state.source !== source || state.active !== active)
    setState(
      initial(
        source,
        active,
        state.source === source ? state.query : '',
        state.source === source ? state.retryAt : 0,
      ),
    );
  const { query, cursor, revision, notBefore } = state;
  useEffect(() => {
    if (!active || !source) return;
    const request = new AbortController();
    const timer = setTimeout(
      () => {
        void Promise.resolve()
          .then(() => {
            request.signal.throwIfAborted();
            return source.searchSkills(query, { cursor, signal: request.signal });
          })
          .then((page) => {
            if (request.signal.aborted) return;
            setState((previous) => {
              if (request.signal.aborted) return previous;
              if (
                page.next !== undefined &&
                (page.next === cursor || previous.seen.includes(page.next))
              )
                return {
                  ...previous,
                  phase: 'failed',
                  failure: new ProtocolError('Skill catalog cursor did not advance.'),
                  retryAt: 0,
                };
              const entries = new Map(
                (cursor === undefined ? [] : previous.entries).map((entry) => [entry.id, entry]),
              );
              for (const entry of page.items)
                if (!entries.has(entry.id)) entries.set(entry.id, entry);
              return {
                ...previous,
                entries: [...entries.values()],
                next: page.next,
                seen: cursor === undefined ? [] : [...previous.seen, cursor],
                phase: 'ready',
                loaded: true,
                failure: undefined,
                retryAt: 0,
              };
            });
          })
          .catch((failure: unknown) => {
            if (request.signal.aborted) return;
            const delay = rateLimitDelay(
              failure,
              failure instanceof ApiError && failure.status === 429 ? 1000 : 0,
            );
            setState((previous) => ({
              ...previous,
              phase: 'failed',
              failure,
              retryAt: delay ? Date.now() + delay : 0,
            }));
          });
      },
      Math.max(200, notBefore - Date.now()),
    );
    return () => {
      request.abort();
      clearTimeout(timer);
    };
  }, [source, active, query, cursor, revision, notBefore]);
  useEffect(() => {
    if (!state.retryAt) return;
    const timer = setTimeout(
      () => setState((previous) => ({ ...previous, retryAt: 0 })),
      Math.max(0, state.retryAt - Date.now()),
    );
    return () => clearTimeout(timer);
  }, [state.retryAt]);
  const waiting = state.retryAt !== 0;
  const blocked = state.failure instanceof ApiError && [401, 403].includes(state.failure.status);
  const searchEnabled = active && source !== undefined && state.loaded && !waiting && !blocked;
  return {
    ...state,
    waiting,
    searchEnabled,
    selectable: active && source !== undefined && state.phase === 'ready',
    search(value: string) {
      if (!searchEnabled || value === state.query) return;
      setState((previous) => ({
        ...initial(source, active, value),
        loaded: true,
        revision: previous.revision + 1,
      }));
    },
    more() {
      if (!active || !source || state.phase !== 'ready' || !state.next || waiting) return;
      setState((previous) => ({
        ...previous,
        cursor: previous.next,
        phase: 'loading',
        failure: undefined,
        revision: previous.revision + 1,
      }));
    },
    retry() {
      if (!active || !source || state.phase !== 'failed' || waiting || blocked) return;
      setState((previous) => ({
        ...previous,
        phase: 'loading',
        failure: undefined,
        revision: previous.revision + 1,
      }));
    },
  };
}
