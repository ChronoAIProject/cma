import type { V2AgentEventItem } from '@chronoai/cma-sdk';
import type { CmaWebClient } from '../../api/client';
import { conversationFromTranscript } from '../conversation';
import { transientReadFailure, type ReadyAgent } from '../loadAgent';
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
): Promise<ReadyAgent & { readonly durableEvents: readonly [] }> {
  await v2.authorize(signal);
  const [detail, usage] = await Promise.all([
    client.readAgent(agentId, { signal }),
    client.readAgentUsage(agentId, { signal }),
  ]);
  if (detail.id !== agentId || usage.agentId !== agentId)
    throw new Error('Agent projections changed identity.');
  const workspace = await client.readWorkspace(detail.workspace.id, { signal });
  if (workspace.id !== detail.workspace.id || workspace.sandboxId !== detail.sandbox.id)
    throw new Error('Agent projections changed identity.');
  let conversation: ConversationState = {
    ...conversationFromTranscript({ agentId, throughSeq: 0, turns: [] }),
    usage,
  };
  const items: V2AgentEventItem[] = [];
  let pageCursor: string | undefined;
  let incarnation: string | undefined;
  let historyUnavailable = false;
  do {
    let page;
    try {
      await v2.authorize(signal);
      page = await v2.events.snapshot(agentId, { pageCursor, signal });
    } catch (error) {
      if (signal.aborted || !allowIncomplete || !transientReadFailure(error)) throw error;
      await v2.authorize(signal);
      historyUnavailable = true;
      break;
    }
    await v2.authorize(signal);
    if (incarnation !== undefined && incarnation !== page.incarnation)
      throw new Error('Agent snapshot changed conversation identity.');
    incarnation = page.incarnation;
    conversation = { ...conversation, agentIncarnation: incarnation };
    for (const item of page.items) conversation = foldV2Item(conversation, item);
    items.push(...page.items);
    pageCursor = page.next_page_cursor ?? undefined;
    if (pageCursor === undefined)
      conversation = { ...conversation, agentCursor: page.through_cursor };
    else
      onProgress?.({
        detail,
        workspace,
        conversation,
        v2,
        v2Items: [...items],
        historyLoading: true,
      });
  } while (pageCursor !== undefined);
  return {
    detail,
    workspace,
    conversation,
    v2,
    v2Items: items,
    durableEvents: [],
    historyLoading: false,
    historyUnavailable,
  };
}
