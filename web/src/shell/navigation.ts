import { useCallback, useEffect, useState } from 'react';
import { ApiError } from '@chronoai/cma-sdk';
import type { CmaWebClient } from '../api/client';
import type { SandboxDetail, SandboxSummary, WorkspaceDetail } from '../api/types';
import { sandboxRefreshDelay } from '../sandbox/lifecycle';

export interface NavigationSandbox {
  readonly sandbox: SandboxDetail;
  readonly workspaces: readonly WorkspaceDetail[];
}

export type NavigationState =
  | { readonly status: 'loading' }
  | {
      readonly status: 'ready';
      readonly items: readonly NavigationSandbox[];
      readonly refreshError?: string;
    }
  | { readonly status: 'error'; readonly message: string };

function unchangedRetiredSandbox(summary: SandboxSummary, previous: SandboxDetail): boolean {
  return (
    summary.status === 'retired' &&
    previous.status === 'retired' &&
    (
      [
        'id',
        'name',
        'createdAt',
        'updatedAt',
        'lastActivityAt',
        'workspaceCount',
        'worktreeCount',
        'agentCount',
      ] as const
    ).every((key) => summary[key] === previous[key]) &&
    JSON.stringify(summary.profile) === JSON.stringify(previous.profile) &&
    JSON.stringify(summary.lifecycle) === JSON.stringify(previous.lifecycle)
  );
}

async function allSandboxes(client: CmaWebClient, signal: AbortSignal) {
  const items: SandboxSummary[] = [];
  const seen = new Set<string>();
  let cursor: string | undefined;
  do {
    const page = await client.listSandboxes({ cursor, signal });
    items.push(...page.items);
    cursor = page.next;
    if (cursor !== undefined && seen.has(cursor)) {
      throw new Error('Sandbox pagination did not advance.');
    }
    if (cursor !== undefined) seen.add(cursor);
  } while (cursor !== undefined);
  return items;
}

export async function loadNavigation(
  client: CmaWebClient,
  signal: AbortSignal,
  previous: readonly NavigationSandbox[] = [],
): Promise<readonly NavigationSandbox[]> {
  const summaries = await allSandboxes(client, signal);
  signal.throwIfAborted();
  const cached = new Map(previous.map((item) => [item.sandbox.id, item]));
  return Promise.all(
    summaries.map(async (summary) => {
      const known = cached.get(summary.id);
      if (known !== undefined && unchangedRetiredSandbox(summary, known.sandbox)) return known;
      const sandbox = await client.readSandbox(summary.id, { signal });
      signal.throwIfAborted();
      if (sandbox.id !== summary.id)
        throw new Error('Sandbox hierarchy referred to different resources.');
      const workspaces = await Promise.all(
        sandbox.workspaces.map(async (summary) => {
          const workspace = await client.readWorkspace(summary.id, { signal });
          if (workspace.id !== summary.id || workspace.sandboxId !== sandbox.id)
            throw new Error('Workspace hierarchy referred to different resources.');
          return workspace;
        }),
      );
      return { sandbox, workspaces };
    }),
  );
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : 'The Sandbox hierarchy could not be loaded.';
}

export function useNavigation(client: CmaWebClient): NavigationState & { refresh: () => void } {
  const [revision, setRevision] = useState(0);
  const [state, setState] = useState<NavigationState>({ status: 'loading' });

  useEffect(() => {
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let inFlight = false;
    let previous: readonly NavigationSandbox[] = [];
    let revalidatedAt = 0;
    const poll = () => {
      if (controller.signal.aborted || inFlight || document.hidden) return;
      clearTimeout(timer);
      inFlight = true;
      // The authorized list stays live; terminal subtrees get a bounded detail recheck.
      const revalidate = Date.now() - revalidatedAt >= 60000;
      void loadNavigation(client, controller.signal, revalidate ? [] : previous)
        .then((items) => {
          if (controller.signal.aborted) return;
          previous = items;
          if (revalidate) revalidatedAt = Date.now();
          setState({ status: 'ready', items });
          timer = setTimeout(
            poll,
            Math.min(5000, ...items.map(({ sandbox }) => sandboxRefreshDelay(sandbox))),
          );
        })
        .catch((error: unknown) => {
          if (controller.signal.aborted) return;
          const denied = error instanceof ApiError && [401, 403, 404].includes(error.status);
          if (denied) previous = [];
          setState((current) =>
            current.status === 'ready' && !denied
              ? { ...current, refreshError: 'Sandbox status could not be refreshed. Reconnecting.' }
              : { status: 'error', message: messageOf(error) },
          );
          timer = setTimeout(poll, 5000);
        })
        .finally(() => {
          inFlight = false;
        });
    };
    document.addEventListener('visibilitychange', poll);
    poll();
    return () => {
      controller.abort();
      clearTimeout(timer);
      document.removeEventListener('visibilitychange', poll);
    };
  }, [client, revision]);

  const refresh = useCallback(() => {
    setRevision((value) => value + 1);
  }, []);

  return { ...state, refresh };
}
