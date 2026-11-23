import { useLayoutEffect, useMemo, useState } from 'react';
import type { CmaWebClient, ReadOptions } from './api/client';
import type { AgentDetail } from './api/types';
import { coalesceDetailReads } from './api/detailReads';
import type { DeleteAction, DeletionRequest } from './deletion';
import { forgetDeletion, restoreDeletions } from './deletionRecovery';

function targetId(action: DeleteAction) {
  return action.kind === 'delete_sandbox'
    ? action.sandboxId
    : action.kind === 'remove_workspace'
      ? action.workspaceId
      : action.agentId;
}

export function sameDeletionRequest(left: DeletionRequest, right: DeletionRequest) {
  return (
    left.key === right.key &&
    left.action.kind === right.action.kind &&
    targetId(left.action) === targetId(right.action)
  );
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
  return requests.filter(
    (request) => !request.accepted || !settled.some((item) => sameDeletionRequest(item, request)),
  );
}

interface ReadAuthority {
  readonly source: CmaWebClient;
  readonly viewerId: string;
  readonly authEpoch: number;
  readonly authorized: boolean;
  readonly selectedAgentId?: string;
}

export interface SelectedRetiredAgent {
  readonly detail: AgentDetail;
  readonly viewerId: string;
  readonly authEpoch: number;
}

function hasRetiredParent(detail: AgentDetail, id: string) {
  return (
    detail.id === id &&
    detail.worktree.agentId === id &&
    detail.worktree.workspaceId === detail.workspace.id &&
    detail.worktree.sandboxId === detail.sandbox.id &&
    detail.sandbox.status === 'retired'
  );
}

/** Observe existing reads only; a missing resource is never proof of completed cleanup. */
function observeAcceptedDeletions(
  source: CmaWebClient,
  viewerId: string,
  onSettled: (request: DeletionRequest) => void,
  onAgent: (authority: ReadAuthority, detail?: AgentDetail) => void,
) {
  let active = false;
  let authority: ReadAuthority | undefined;
  const matching = (kind: DeleteAction['kind'], id: string) =>
    restoreDeletions(viewerId).filter(
      (request) => request.action.kind === kind && targetId(request.action) === id,
    );
  const isCurrent = (permission: ReadAuthority | undefined, options?: ReadOptions) =>
    active && permission?.authorized && authority === permission && !options?.signal?.aborted;
  const settle = (request: DeletionRequest) => {
    const current = matching(request.action.kind, targetId(request.action));
    if (current.length === 1 && current[0]?.key === request.key && current[0].accepted) {
      forgetDeletion(viewerId, request.key);
      onSettled(request);
    }
  };
  const observe =
    <T>(
      kind: DeleteAction['kind'],
      read: (id: string, options?: ReadOptions) => Promise<T>,
      complete: (value: T, id: string) => boolean,
    ) =>
    async (id: string, options?: ReadOptions): Promise<T> => {
      const permission = authority;
      const before = matching(kind, id);
      const request = before.length === 1 && before[0]?.accepted ? before[0] : undefined;
      const value = await read(id, options);
      if (request && isCurrent(permission, options) && complete(value, id)) settle(request);
      return value;
    };
  const readAgent = observe(
    'delete_agent',
    source.readAgent.bind(source),
    (value, id) =>
      value.id === id &&
      value.status === 'deleted' &&
      value.worktree.agentId === id &&
      value.worktree.status === 'removed',
  );
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
    async readAgent(id, options) {
      const permission = authority;
      const before = restoreDeletions(viewerId).filter(
        (request) => request.action.kind === 'delete_sandbox',
      );
      try {
        const detail = await readAgent(id, options);
        if (isCurrent(permission, options)) {
          const retired = hasRetiredParent(detail, id);
          if (permission?.selectedAgentId === id) onAgent(permission, retired ? detail : undefined);
          const parent = before.filter((request) => targetId(request.action) === detail.sandbox.id);
          if (retired && parent.length === 1 && parent[0]?.accepted) settle(parent[0]);
        }
        return detail;
      } catch (error) {
        if (isCurrent(permission, options) && permission?.selectedAgentId === id)
          onAgent(permission);
        throw error;
      }
    },
  };

  const reads = coalesceDetailReads(client);
  return {
    client: reads.client,
    authorize(permission: ReadAuthority) {
      authority = permission;
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
  selectedAgentId?: string,
) {
  const [settled, setSettled] = useState<DeletionSettlements>();
  const permission = useMemo(
    () => ({ source, viewerId, authEpoch, authorized, selectedAgentId }),
    [source, viewerId, authEpoch, authorized, selectedAgentId],
  );
  const [observedAgent, setObservedAgent] = useState<{
    permission: ReadAuthority;
    detail?: AgentDetail;
  }>();
  const scope = useMemo(() => {
    const observed = observeAcceptedDeletions(
      source,
      viewerId,
      (request) =>
        setSettled((current) => ({
          client: observed.client,
          viewerId,
          requests: [...(current?.client === observed.client ? current.requests : []), request],
        })),
      (permission, detail) => setObservedAgent({ permission, detail }),
    );
    return observed;
  }, [source, viewerId]);
  useLayoutEffect(() => {
    scope.authorize(permission);
  }, [scope, permission]);
  useLayoutEffect(() => scope.activate(), [scope]);
  const settlements = settled?.client === scope.client ? settled : undefined;
  const selectedRetired: SelectedRetiredAgent | undefined =
    authorized && observedAgent?.permission === permission && observedAgent.detail
      ? { detail: observedAgent.detail, viewerId, authEpoch }
      : undefined;
  return {
    client: scope.client,
    revision: settlements?.requests.length ?? 0,
    settlements,
    selectedRetired,
  };
}
