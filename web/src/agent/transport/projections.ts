import type { CmaWebClient } from '../../api/client';
import type { AgentLoadLineage } from '../loadAgent';

/** Call only after fresh access/identity admission; publication needs its own proof. */
export function readAgentProjections(
  client: CmaWebClient,
  agentId: string,
  signal: AbortSignal,
  lineage?: AgentLoadLineage,
) {
  signal.throwIfAborted();
  const hierarchy = (async () => {
    const detailRead = (async () => client.readAgent(agentId, { signal }))();
    const [detail, preparedWorkspace] = await Promise.all([
      detailRead,
      lineage?.agentId === agentId
        ? (async () => client.readWorkspace(lineage.workspaceId, { signal }))()
        : undefined,
    ]);
    signal.throwIfAborted();
    if (
      detail.id !== agentId ||
      (lineage !== undefined &&
        (lineage.agentId !== agentId ||
          lineage.workspaceId !== detail.workspace.id ||
          lineage.sandboxId !== detail.sandbox.id))
    )
      throw new Error('Agent projections changed identity.');
    const workspace =
      preparedWorkspace ?? (await client.readWorkspace(detail.workspace.id, { signal }));
    signal.throwIfAborted();
    if (workspace.id !== detail.workspace.id || workspace.sandboxId !== detail.sandbox.id)
      throw new Error('Agent projections changed identity.');
    return { detail, workspace };
  })();
  const transcript = (async () => {
    const value = await client.readAgentTranscript(agentId, { signal });
    signal.throwIfAborted();
    if (value.agentId !== agentId) throw new Error('Agent projections changed identity.');
    return value;
  })();
  const usage = (async () => {
    const value = await client.readAgentUsage(agentId, { signal });
    signal.throwIfAborted();
    if (value.agentId !== agentId) throw new Error('Agent projections changed identity.');
    return value;
  })();
  // A route can be cancelled before its loader attaches. Observe every failure
  // now; the original promises still reject when their consumer awaits them.
  void Promise.allSettled([hierarchy, transcript, usage]);
  return { hierarchy, transcript, usage };
}

export type AgentProjectionReads = ReturnType<typeof readAgentProjections>;
