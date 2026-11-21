import type { CmaWebClient } from '../api/client';
import type { Dispatch, RefObject, SetStateAction } from 'react';
import type { AgentResourceState } from './controllerTypes';
import { loadAgent } from './loadAgent';
import { loadV2Agent } from './transport/load';
import { ReadinessWake, readinessDelay } from './readinessWait';

/** Observe startup without repeatedly loading the still-empty conversation. */
export async function readPreparation(
  client: CmaWebClient,
  agentId: string,
  signal: AbortSignal,
  state: RefObject<AgentResourceState>,
  publish: Dispatch<SetStateAction<AgentResourceState>>,
  wake = new ReadinessWake(),
) {
  const current = state.current;
  if (current.status !== 'ready' || current.detail.id !== agentId)
    return loadAgent(client, agentId, signal);
  const lineage = {
    agentId,
    workspaceId: current.detail.workspace.id,
    sandboxId: current.detail.sandbox.id,
  };
  let publishPreparation: (() => void) | undefined;
  // V2 has durable replay for an active first message. Retained legacy tabs
  // still need their JSON reconciliation while first-message delivery is pending.
  if (
    current.detail.status === 'starting' ||
    (current.v2 !== undefined && current.detail.firstMessage !== undefined)
  ) {
    let attempt = 0;
    for (;;) {
      if (document.hidden) await wake.wait(0, signal);
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
      if (
        detail.status !== current.detail.status ||
        detail.sandbox.status !== current.detail.sandbox.status ||
        JSON.stringify(detail.firstMessage) !== JSON.stringify(current.detail.firstMessage)
      ) {
        // Pending first-message work keeps the preparation effect alive while
        // history refreshes. Settled preparation publishes only the full read.
        if (
          detail.firstMessage !== undefined &&
          !['completed', 'failed', 'recovery_required'].includes(detail.firstMessage.state)
        ) {
          publishPreparation = () => {
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
          };
        }
        break;
      }
      await wake.wait(readinessDelay(attempt++), signal);
    }
  }
  // Keep the selected transport and journal, with a fresh handle so live replay
  // restarts at the new snapshot cursor. Loading still checks current authority.
  if (current.v2 === undefined) {
    publishPreparation?.();
    return loadAgent(client, agentId, signal, false, undefined, lineage);
  }
  return loadV2Agent(
    client,
    agentId,
    signal,
    current.v2.forSnapshotRefresh(),
    false,
    undefined,
    lineage,
    undefined,
    publishPreparation,
  );
}
