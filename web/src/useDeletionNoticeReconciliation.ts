import { useLayoutEffect, useMemo, useState } from 'react';
import type { CmaWebClient, ReadOptions } from './api/client';
import { coalesceDetailReads } from './api/detailReads';
import type { DeleteAction, DeletionRequest } from './deletion';
import { forgetDeletion, restoreDeletions, sameDeletionRequest } from './deletionRecovery';

function targetId(action: DeleteAction) {
  return action.kind === 'delete_sandbox'
    ? action.sandboxId
    : action.kind === 'remove_workspace'
      ? action.workspaceId
      : action.agentId;
}

const completedRequests = new WeakMap<CmaWebClient, DeletionRequest[]>();

export function deletionWasSettled(client: CmaWebClient, request: DeletionRequest): boolean {
  return completedRequests.get(client)?.some((item) => sameDeletionRequest(item, request)) ?? false;
}

export interface DeletionSettlements {
  readonly client: CmaWebClient;
  readonly viewerId: string;
  readonly requests: readonly DeletionRequest[];
}

export function reconcileDeletionRequests(
  memory: readonly DeletionRequest[],
  persisted: readonly DeletionRequest[],
  settled: readonly DeletionRequest[] = [],
): readonly DeletionRequest[] {
  const requests = [
    ...memory,
    ...persisted.filter((request) => !memory.some((item) => sameDeletionRequest(item, request))),
  ];
  return requests.filter((request) => !settled.some((item) => sameDeletionRequest(item, request)));
}

/** Observe existing reads only; a missing resource is never proof of completed cleanup. */
function observeAcceptedDeletions(
  source: CmaWebClient,
  viewerId: string,
  onSettled: (request: DeletionRequest) => void,
) {
  let active = false;
  const completed: DeletionRequest[] = [];
  let authority: { authEpoch: number; authorized: boolean } | undefined;
  const matching = (kind: DeleteAction['kind'], id: string) =>
    restoreDeletions(viewerId).filter(
      (request) => request.action.kind === kind && targetId(request.action) === id,
    );
  const observe =
    <T>(
      kind: DeleteAction['kind'],
      read: (id: string, options?: ReadOptions) => Promise<T>,
      complete: (value: T, id: string, request: DeletionRequest) => boolean,
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
        complete(value, id, request)
      ) {
        const current = matching(kind, id);
        if (
          current.length === 1 &&
          current[0]?.accepted &&
          sameDeletionRequest(current[0], request)
        ) {
          completed.push(request);
          forgetDeletion(viewerId, request.key);
          onSettled(request);
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
      (value, id, request) =>
        (!request.stop ||
          (value.sandbox.id === request.stop.sandboxId &&
            value.workspace.id === request.stop.workspaceId &&
            value.worktree.id === request.stop.worktreeId &&
            value.worktree.sandboxId === request.stop.sandboxId &&
            value.worktree.workspaceId === request.stop.workspaceId)) &&
        value.id === id &&
        value.status === 'deleted' &&
        value.worktree.agentId === id &&
        value.worktree.status === 'removed',
    ),
  };

  const reads = coalesceDetailReads(client);
  completedRequests.set(reads.client, completed);
  completedRequests.set(client, completed);
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
  const [settled, setSettled] = useState<DeletionSettlements>();
  const scope = useMemo(() => {
    const observed = observeAcceptedDeletions(source, viewerId, (request) =>
      setSettled((current) => ({
        client: observed.client,
        viewerId,
        requests: [...(current?.client === observed.client ? current.requests : []), request],
      })),
    );
    return observed;
  }, [source, viewerId]);
  useLayoutEffect(() => {
    scope.authorize(authEpoch, authorized);
  }, [scope, authEpoch, authorized]);
  useLayoutEffect(() => scope.activate(), [scope]);
  const settlements = settled?.client === scope.client ? settled : undefined;
  return { client: scope.client, revision: settlements?.requests.length ?? 0, settlements };
}
