import type { CmaWebClient } from '../api/client';
import type { AgentLoadLineage, ReadyAgent } from './loadAgent';

/** Reopening a complete view refreshes metadata; replay resumes at its saved cursor. */
export async function reopenAgent(
  client: CmaWebClient,
  retained: ReadyAgent,
  signal: AbortSignal,
  lineage?: AgentLoadLineage,
): Promise<ReadyAgent> {
  await retained.v2?.authorize(signal);
  const [detail, workspace] = await Promise.all([
    client.readAgent(retained.detail.id, { signal }),
    client.readWorkspace(retained.workspace.id, { signal }),
  ]);
  signal.throwIfAborted();
  if (
    detail.id !== retained.detail.id ||
    detail.workspace.id !== workspace.id ||
    workspace.id !== retained.workspace.id ||
    workspace.sandboxId !== detail.sandbox.id ||
    detail.sandbox.id !== retained.detail.sandbox.id ||
    (lineage !== undefined &&
      (lineage.agentId !== detail.id ||
        lineage.workspaceId !== workspace.id ||
        lineage.sandboxId !== detail.sandbox.id))
  )
    throw new Error('Agent projections changed identity.');
  await retained.v2?.authorize(signal);
  return { ...retained, detail, workspace };
}
