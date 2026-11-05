import type { CmaWebClient } from '../api/client';
import type { AgentLoadLineage, ReadyAgent } from './loadAgent';
import { transientReadFailure } from './loadAgent';
import { conversationFromTranscript } from './conversation';
import {
  conversationFromCurrentState,
  conversationFromWindow,
  withCurrentState,
} from './historyWindow';
import type { AgentV2Session } from './transport/client';

export async function loadBoundedAgent(
  client: CmaWebClient,
  agentId: string,
  signal: AbortSignal,
  v2: AgentV2Session | undefined,
  allowIncomplete: boolean,
  lineage?: AgentLoadLineage,
  onInputReady?: (ready: ReadyAgent) => void,
): Promise<ReadyAgent & { readonly durableEvents: readonly [] }> {
  const attempt = new AbortController();
  try {
    return await readBoundedAgent(
      client,
      agentId,
      AbortSignal.any([signal, attempt.signal]),
      v2,
      allowIncomplete,
      lineage,
      onInputReady,
    );
  } finally {
    // A failed state/history read must not publish another parallel result after
    // its caller has already selected the predecessor fallback.
    attempt.abort();
  }
}

async function readBoundedAgent(
  client: CmaWebClient,
  agentId: string,
  signal: AbortSignal,
  v2: AgentV2Session | undefined,
  allowIncomplete: boolean,
  lineage?: AgentLoadLineage,
  onInputReady?: (ready: ReadyAgent) => void,
): Promise<ReadyAgent & { readonly durableEvents: readonly [] }> {
  const events = client.agentHistory!;
  await v2?.authorize(signal);
  signal.throwIfAborted();
  const hierarchy = (async () => {
    const [detail, hinted] = await Promise.all([
      client.readAgent(agentId, { signal }),
      lineage?.agentId === agentId
        ? client.readWorkspace(lineage.workspaceId, { signal })
        : undefined,
    ]);
    const workspace = hinted ?? (await client.readWorkspace(detail.workspace.id, { signal }));
    signal.throwIfAborted();
    if (
      detail.id !== agentId ||
      workspace.id !== detail.workspace.id ||
      workspace.sandboxId !== detail.sandbox.id ||
      (lineage !== undefined &&
        (lineage.agentId !== agentId ||
          lineage.workspaceId !== workspace.id ||
          lineage.sandboxId !== detail.sandbox.id))
    )
      throw new Error('Agent projections changed identity.');
    await v2?.authorize(signal);
    signal.throwIfAborted();
    onInputReady?.({
      detail,
      workspace,
      v2,
      conversation: conversationFromTranscript({ agentId, throughSeq: 0, turns: [] }),
      historyLoading: true,
    });
    return { detail, workspace };
  })();
  const controls = events.state(agentId, { signal });
  const history = controls.then(async (current) => {
    const metadata = await hierarchy;
    await v2?.authorize(signal);
    signal.throwIfAborted();
    onInputReady?.({
      ...metadata,
      v2,
      conversation: conversationFromCurrentState(current),
      historyLoading: true,
      replayReady: true,
    });
    try {
      const page = await events.history(agentId, {
        throughCursor: current.through_cursor,
        limit: 200,
        toolOutput: true,
        messageBindings: true,
        signal,
      });
      return withCurrentState(conversationFromWindow(page), current);
    } catch (error) {
      if (signal.aborted || !allowIncomplete || !transientReadFailure(error)) throw error;
      return undefined;
    }
  });
  const [metadata, current, conversation, usage] = await Promise.all([
    hierarchy,
    controls,
    history,
    client.readAgentUsage(agentId, { signal }),
  ]);
  signal.throwIfAborted();
  await v2?.authorize(signal);
  if (usage.agentId !== agentId) throw new Error('Agent projections changed identity.');
  return {
    ...metadata,
    v2,
    conversation: { ...(conversation ?? conversationFromCurrentState(current)), usage },
    durableEvents: [],
    historyLoading: false,
    historyUnavailable: conversation === undefined,
    replayReady: true,
  };
}
