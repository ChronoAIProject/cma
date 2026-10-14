import { useEffect, useState } from 'react';
import type { AgentLifecycleOperationView, CmaWebClient } from '../api/client';
import type { AgentDetail } from '../api/types';

// The creation receipt identifies an operation. Only its admitted Agent may open early.
export function useLifecycleAgent(
  client: CmaWebClient,
  operation: AgentLifecycleOperationView | undefined,
  receiptId: string | undefined,
  viewerId: string | undefined,
  workspaceId: string | undefined,
  sandboxId: string | undefined,
) {
  const operationId = operation?.id;
  const agentId = operation?.agent_id;
  const eligible = operation?.status === 'running' && operationId === receiptId;
  const [observed, setObserved] = useState<{ operationId: string; agent: AgentDetail }>();
  useEffect(() => {
    if (!eligible || !operationId || !agentId || !viewerId) return;
    const scope = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    const poll = async () => {
      const request = new AbortController();
      const signal = AbortSignal.any([scope.signal, request.signal]);
      let deadline: ReturnType<typeof setTimeout> | undefined;
      try {
        const agent = await Promise.race([
          (async () => {
            const detail = await client.readAgent(agentId, { signal });
            if (
              signal.aborted ||
              detail.id !== agentId ||
              !['starting', 'ready', 'working'].includes(detail.status) ||
              (workspaceId !== undefined && detail.workspace.id !== workspaceId) ||
              (sandboxId !== undefined && detail.sandbox.id !== sandboxId) ||
              detail.worktree.agentId !== agentId ||
              detail.worktree.workspaceId !== detail.workspace.id ||
              detail.worktree.sandboxId !== detail.sandbox.id
            )
              return undefined;
            const parent = await client.readSandbox(detail.sandbox.id, { signal });
            if (
              signal.aborted ||
              parent.id !== detail.sandbox.id ||
              (parent.owner !== undefined && parent.owner !== viewerId) ||
              parent.status !== 'active' ||
              parent.lifecycle === undefined ||
              parent.lifecycle.operation !== undefined ||
              parent.lifecycle.checkpoint !== undefined
            )
              return undefined;
            return detail;
          })(),
          new Promise<undefined>((resolve) => {
            deadline = setTimeout(() => {
              request.abort();
              resolve(undefined);
            }, 5000);
          }),
        ]);
        if (scope.signal.aborted) return;
        if (agent !== undefined) {
          setObserved({ operationId, agent });
          return;
        }
      } catch {
        // Lifecycle polling remains the authority; a failed read cannot fabricate readiness.
      } finally {
        clearTimeout(deadline);
        request.abort();
      }
      if (!scope.signal.aborted) timer = setTimeout(() => void poll(), 500);
    };
    void poll();
    return () => {
      scope.abort();
      clearTimeout(timer);
    };
  }, [agentId, client, eligible, operationId, sandboxId, viewerId, workspaceId]);
  return eligible &&
    observed !== undefined &&
    observed.operationId === operationId &&
    observed.agent.id === agentId
    ? observed.agent
    : undefined;
}
