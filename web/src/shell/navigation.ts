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
): Promise<readonly NavigationSandbox[]> {
  const summaries = await allSandboxes(client, signal);
  signal.throwIfAborted();
  const items = await Promise.all(
    summaries
      .filter((summary) => summary.status !== 'retired')
      .map(async (summary) => {
        const sandbox = await client.readSandbox(summary.id, { signal });
        signal.throwIfAborted();
        if (sandbox.id !== summary.id)
          throw new Error('Sandbox hierarchy referred to different resources.');
        if (sandbox.status === 'retired') return undefined;
        if (sandbox.status === 'retiring') return { sandbox, workspaces: [] };
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
  return items.filter((item) => item !== undefined);
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
    const poll = () => {
      if (controller.signal.aborted || inFlight || document.hidden) return;
      clearTimeout(timer);
      inFlight = true;
      void loadNavigation(client, controller.signal)
        .then((items) => {
          if (controller.signal.aborted) return;
          setState({ status: 'ready', items });
          timer = setTimeout(
            poll,
            Math.min(5000, ...items.map(({ sandbox }) => sandboxRefreshDelay(sandbox))),
          );
        })
        .catch((error: unknown) => {
          if (controller.signal.aborted) return;
          const denied = error instanceof ApiError && [401, 403, 404].includes(error.status);
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
