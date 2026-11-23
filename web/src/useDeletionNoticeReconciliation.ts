import { useLayoutEffect, useMemo, useState } from 'react';
import type { CmaWebClient, ReadOptions } from './api/client';
import { coalesceDetailReads } from './api/detailReads';
import type { DeleteAction } from './deletion';
import { forgetDeletion, restoreDeletions } from './deletionRecovery';

function sameTarget(action: DeleteAction, kind: DeleteAction['kind'], id: string) {
  return (
    action.kind === kind &&
    (action.kind === 'delete_sandbox'
      ? action.sandboxId === id
      : action.kind === 'remove_workspace'
        ? action.workspaceId === id
        : action.agentId === id)
  );
}

/** Observe existing reads only; a missing resource is never proof of completed cleanup. */
function observeAcceptedDeletions(source: CmaWebClient, viewerId: string, onSettled: () => void) {
  let active = false;
  let authority: { authEpoch: number; authorized: boolean } | undefined;
  const matching = (kind: DeleteAction['kind'], id: string) =>
    restoreDeletions(viewerId).filter((request) => sameTarget(request.action, kind, id));
  const observe =
    <T>(
      kind: DeleteAction['kind'],
      read: (id: string, options?: ReadOptions) => Promise<T>,
      complete: (value: T, id: string) => boolean,
    ) =>
    async (id: string, options?: ReadOptions): Promise<T> => {
      const permission = authority;
      const before = matching(kind, id);
      const request =
        active && permission?.authorized && before.length === 1 && before[0]?.accepted
          ? before[0]
          : undefined;
      const value = await read(id, options);
      if (
        request &&
        active &&
        authority === permission &&
        !options?.signal?.aborted &&
        complete(value, id)
      ) {
        const current = matching(kind, id);
        if (current.length === 1 && current[0]?.key === request.key && current[0].accepted) {
          forgetDeletion(viewerId, request.key);
          onSettled();
        }
      }
      return value;
    };
  const client: CmaWebClient = {
    ...source,
    readSandbox: observe(
      'delete_sandbox',
      source.readSandbox.bind(source),
      (value, id) => value.id === id && value.status === 'retired',
    ),
    readWorkspace: observe(
      'remove_workspace',
      source.readWorkspace.bind(source),
      (value, id) => value.id === id && value.status === 'removed',
    ),
    readAgent: observe(
      'delete_agent',
      source.readAgent.bind(source),
      (value, id) =>
        value.id === id &&
        value.status === 'deleted' &&
        value.worktree.agentId === id &&
        value.worktree.status === 'removed',
    ),
  };

  const reads = coalesceDetailReads(client);
  return {
    client: reads.client,
    authorize(authEpoch: number, authorized: boolean) {
      authority = { authEpoch, authorized };
    },
    activate() {
      active = true;
      return () => {
        active = false;
        reads.dispose();
      };
    },
  };
}

export function useDeletionNoticeReconciliation(
  source: CmaWebClient,
  viewerId: string,
  authEpoch: number,
  authorized: boolean,
) {
  const [revision, setRevision] = useState(0);
  const scope = useMemo(
    () => observeAcceptedDeletions(source, viewerId, () => setRevision((value) => value + 1)),
    [source, viewerId],
  );
  useLayoutEffect(() => {
    scope.authorize(authEpoch, authorized);
  }, [scope, authEpoch, authorized]);
  useLayoutEffect(() => scope.activate(), [scope]);
  return { client: scope.client, revision };
}
