import { useLayoutEffect, useMemo, useState } from 'react';
import type { CmaWebClient, ReadOptions } from './api/client';
import type { AgentDetail } from './api/types';
import { coalesceDetailReads } from './api/detailReads';
import { agentDetail } from './api/rest/hierarchy';
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

function withClientProperty<T extends object, K extends keyof T>(
  source: T,
  key: K,
  value: T[K],
): T {
  const methods = new Map<unknown, unknown>();
  return new Proxy(source, {
    get(target, property) {
      if (property === key) return value;
      const original: unknown = Reflect.get(target, property, target);
      if (typeof original !== 'function') return original;
      if (!methods.has(original)) methods.set(original, original.bind(target));
      return methods.get(original);
    },
  });
}

/** Observe existing reads only; a missing resource is never proof of completed cleanup. */
function observeAcceptedDeletions(
  source: CmaWebClient,
  viewerId: string,
  onSettled: (request: DeletionRequest) => void,
  onAgent: (authority: ReadAuthority, detail?: AgentDetail) => void,
) {
  let active = false;
  const completed: DeletionRequest[] = [];
  let authority: ReadAuthority | undefined;
  const latestAgentReads = new Map<string, object>();
  const matching = (kind: DeleteAction['kind'], id: string) =>
    restoreDeletions(viewerId).filter(
      (request) => request.action.kind === kind && targetId(request.action) === id,
    );
  const isCurrent = (permission: ReadAuthority | undefined, options?: ReadOptions) =>
    active && permission?.authorized && authority === permission && !options?.signal?.aborted;
  const settle = (request: DeletionRequest) => {
    const current = matching(request.action.kind, targetId(request.action));
    if (current.length === 1 && current[0]?.accepted && sameDeletionRequest(current[0], request)) {
      completed.push(request);
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
  const observeAgent =
    <T>(
      read: (id: string, options?: ReadOptions) => Promise<T>,
      project: (value: T) => AgentDetail | undefined,
      settleAgent = false,
    ) =>
    async (id: string, options?: ReadOptions): Promise<T> => {
      const permission = authority;
      const before = restoreDeletions(viewerId);
      const token = {};
      latestAgentReads.set(id, token);
      const current = () => isCurrent(permission, options) && latestAgentReads.get(id) === token;
      try {
        const value = await read(id, options);
        if (current()) {
          const detail = project(value);
          const retired = detail !== undefined && hasRetiredParent(detail, id);
          if (permission?.selectedAgentId === id) onAgent(permission, retired ? detail : undefined);
          const parent = before.filter(
            (request) =>
              request.action.kind === 'delete_sandbox' &&
              targetId(request.action) === detail?.sandbox.id,
          );
          if (retired && parent.length === 1 && parent[0]?.accepted) settle(parent[0]);
          const agent = before.filter(
            (request) => request.action.kind === 'delete_agent' && targetId(request.action) === id,
          );
          const request = agent.length === 1 && agent[0]?.accepted ? agent[0] : undefined;
          if (
            settleAgent &&
            request &&
            detail?.id === id &&
            detail.status === 'deleted' &&
            detail.worktree.agentId === id &&
            detail.worktree.status === 'removed' &&
            (!request.stop ||
              (detail.sandbox.id === request.stop.sandboxId &&
                detail.workspace.id === request.stop.workspaceId &&
                detail.worktree.id === request.stop.worktreeId &&
                detail.worktree.sandboxId === request.stop.sandboxId &&
                detail.worktree.workspaceId === request.stop.workspaceId))
          )
            settle(request);
        }
        return value;
      } catch (error) {
        if (current() && permission?.selectedAgentId === id) onAgent(permission);
        throw error;
      } finally {
        if (latestAgentReads.get(id) === token) latestAgentReads.delete(id);
      }
    };
  const resources = source.resourceViews;
  const resourceViews =
    resources === undefined
      ? undefined
      : withClientProperty(
          resources,
          'agents',
          withClientProperty(
            resources.agents,
            'readAccessible',
            observeAgent(
              (id, options) => resources.agents.readAccessible(id, options),
              (value) => (value.access === 'shared' ? undefined : agentDetail(value)),
            ),
          ),
        );
  const client: CmaWebClient = {
    ...source,
    resourceViews,
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
    readAgent: observeAgent(source.readAgent.bind(source), (value) => value, true),
  };

  const reads = coalesceDetailReads(client);
  completedRequests.set(reads.client, completed);
  completedRequests.set(client, completed);
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
