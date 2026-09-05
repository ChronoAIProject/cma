import { useCallback, useEffect, useState } from 'react';
import type { CmaWebClient } from '../api/client';
import type { SandboxDetail, SandboxSummary, WorkspaceDetail } from '../api/types';

export interface NavigationSandbox {
  readonly sandbox: SandboxDetail;
  readonly workspaces: readonly WorkspaceDetail[];
}

export type NavigationState =
  | { readonly status: 'loading' }
  | { readonly status: 'ready'; readonly items: readonly NavigationSandbox[] }
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
  return Promise.all(
    summaries.map(async ({ id }) => {
      const sandbox = await client.readSandbox(id, { signal });
      const workspaces = await Promise.all(
        sandbox.workspaces.map((workspace) => client.readWorkspace(workspace.id, { signal })),
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
    loadNavigation(client, controller.signal)
      .then((items) => {
        if (!controller.signal.aborted) setState({ status: 'ready', items });
      })
      .catch((error: unknown) => {
        if (!controller.signal.aborted) setState({ status: 'error', message: messageOf(error) });
      });
    return () => {
      controller.abort();
    };
  }, [client, revision]);

  const refresh = useCallback(() => {
    setState({ status: 'loading' });
    setRevision((value) => value + 1);
  }, []);

  return { ...state, refresh };
}
