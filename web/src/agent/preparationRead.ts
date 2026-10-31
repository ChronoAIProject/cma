import type { CmaWebClient } from '../api/client';
import type { AgentResourceState } from './controllerTypes';
import { loadAgent } from './loadAgent';

/** Observe startup without repeatedly loading the still-empty conversation. */
export async function readPreparation(
  client: CmaWebClient,
  agentId: string,
  signal: AbortSignal,
  current: AgentResourceState,
) {
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
      if (detail.status !== 'starting' || detail.sandbox.status !== current.detail.sandbox.status)
        break;
      await wait(performance.now() - started < 10000 ? 250 : 1500, signal);
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
