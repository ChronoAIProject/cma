import type { CmaWebClient } from '../api/client';
import type { Dispatch, RefObject, SetStateAction } from 'react';
import type { AgentResourceState } from './controllerTypes';
import { loadAgent } from './loadAgent';

/** Observe startup without repeatedly loading the still-empty conversation. */
export async function readPreparation(
  client: CmaWebClient,
  agentId: string,
  signal: AbortSignal,
  state: RefObject<AgentResourceState>,
  publish: Dispatch<SetStateAction<AgentResourceState>>,
) {
  const current = state.current;
  if (current.status !== 'ready' || current.detail.id !== agentId)
    return loadAgent(client, agentId, signal);
  const lineage = {
    agentId,
    workspaceId: current.detail.workspace.id,
    sandboxId: current.detail.sandbox.id,
  };
  if (current.detail.status === 'starting') {
    const started = performance.now();
    for (;;) {
      const readStarted = performance.now();
      await current.v2?.authorize(signal);
      signal.throwIfAborted();
      const detail = await client.readAgent(agentId, { signal });
      signal.throwIfAborted();
      if (
        detail.id !== agentId ||
        detail.workspace.id !== lineage.workspaceId ||
        detail.sandbox.id !== lineage.sandboxId ||
        detail.worktree.id !== current.detail.worktree.id
      )
        throw new Error('Agent projections changed identity.');
      if (detail.status !== 'starting' || detail.sandbox.status !== current.detail.sandbox.status) {
        // Pending first-message work keeps the preparation effect alive while
        // history refreshes. Settled preparation publishes only the full read.
        if (
          detail.firstMessage !== undefined &&
          !['completed', 'failed', 'recovery_required'].includes(detail.firstMessage.state)
        ) {
          await current.v2?.authorize(signal);
          signal.throwIfAborted();
          publish((latest) =>
            latest.status === 'ready' &&
            latest.detail.id === agentId &&
            latest.detail.workspace.id === lineage.workspaceId &&
            latest.detail.sandbox.id === lineage.sandboxId &&
            latest.detail.worktree.id === detail.worktree.id &&
            Date.parse(latest.detail.updatedAt) <= Date.parse(detail.updatedAt)
              ? { ...latest, detail }
              : latest,
          );
        }
        break;
      }
      // Include authorization and the completed read in the short startup cadence.
      // Slow preparation retains its full backoff; requests never overlap.
      const elapsed = performance.now() - readStarted;
      await wait(performance.now() - started < 10000 ? Math.max(0, 250 - elapsed) : 1500, signal);
    }
  }
  return loadAgent(client, agentId, signal, false, undefined, lineage);
}

function wait(milliseconds: number, signal: AbortSignal): Promise<void> {
  signal.throwIfAborted();
  return new Promise((resolve, reject) => {
    const abort = () => {
      clearTimeout(timer);
      signal.removeEventListener('abort', abort);
      reject(signal.reason);
    };
    const timer = setTimeout(() => {
      signal.removeEventListener('abort', abort);
      resolve();
    }, milliseconds);
    signal.addEventListener('abort', abort, { once: true });
  });
}
