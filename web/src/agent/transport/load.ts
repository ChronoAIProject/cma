import type { V2AgentEventItem } from '@chronoai/cma-sdk';
import type { CmaWebClient } from '../../api/client';
import { conversationFromTranscript } from '../conversation';
import { transientReadFailure, type AgentLoadLineage, type ReadyAgent } from '../loadAgent';
import type { ConversationState } from '../model';
import { foldV2Item } from './projection';
import type { AgentV2Session } from './client';

export async function loadV2Agent(
  client: CmaWebClient,
  agentId: string,
  signal: AbortSignal,
  v2: AgentV2Session,
  allowIncomplete = false,
  onProgress?: (ready: ReadyAgent) => void,
  lineage?: AgentLoadLineage,
): Promise<ReadyAgent & { readonly durableEvents: readonly [] }> {
  await v2.authorize(signal);
  const readHierarchy = async () => {
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
  };
  const readUsage = async () => {
    const usage = await client.readAgentUsage(agentId, { signal });
    signal.throwIfAborted();
    if (usage.agentId !== agentId) throw new Error('Agent projections changed identity.');
    return usage;
  };
  const readPage = async (pageCursor?: string) => {
    try {
      // The entry check covers the first parallel batch. Later pages follow
      // awaited reads and progress callbacks, so each needs fresh authority.
      if (pageCursor !== undefined) await v2.authorize(signal);
      signal.throwIfAborted();
      v2.journal.read();
      return await v2.events.snapshot(agentId, { pageCursor, signal });
    } catch (error) {
      if (signal.aborted || !allowIncomplete || !transientReadFailure(error)) throw error;
      return undefined;
    }
  };
  // Independent reads share one load, but no result is exposed until every
  // required projection has settled and the account is checked again.
  const [{ detail, workspace }, usage, firstPage] = await Promise.all([
    readHierarchy(),
    readUsage(),
    readPage(),
  ]);
  await v2.authorize(signal);
  let conversation: ConversationState = {
    ...conversationFromTranscript({ agentId, throughSeq: 0, turns: [] }),
    usage,
  };
  const items: V2AgentEventItem[] = [];
  let incarnation: string | undefined;
  let page = firstPage;
  while (page !== undefined) {
    if (incarnation !== undefined && incarnation !== page.incarnation)
      throw new Error('Agent snapshot changed conversation identity.');
    incarnation = page.incarnation;
    conversation = { ...conversation, agentIncarnation: incarnation };
    for (const item of page.items) conversation = foldV2Item(conversation, item);
    items.push(...page.items);
    const pageCursor = page.next_page_cursor ?? undefined;
    if (pageCursor === undefined) {
      conversation = { ...conversation, agentCursor: page.through_cursor };
      break;
    }
    onProgress?.({
      detail,
      workspace,
      conversation,
      v2,
      v2Items: [...items],
      historyLoading: true,
    });
    page = await readPage(pageCursor);
    await v2.authorize(signal);
  }
  return {
    detail,
    workspace,
    conversation,
    v2,
    v2Items: items,
    durableEvents: [],
    historyLoading: false,
    historyUnavailable: page === undefined,
  };
}
