import { ApiError, TransportError } from '@chronoai/cma-sdk';
import type { CmaWebClient } from '../api/client';
import type {
  AgentDetail,
  AgentEvent,
  AgentUsage,
  SandboxEvent,
  WorkspaceDetail,
} from '../api/types';
import { mergeTranscriptSnapshot } from './chronology';
import { conversationFromTranscript, foldAgentEvent, type ConversationState } from './conversation';
import { loadV2Agent } from './transport/load';
import type { AgentV2Session } from './transport/client';
import type { V2AgentEventItem } from '@chronoai/cma-sdk';

export interface ReadyAgent {
  readonly v2?: AgentV2Session;
  readonly v2Items?: readonly V2AgentEventItem[];
  readonly detail: AgentDetail;
  readonly workspace: WorkspaceDetail;
  readonly conversation: ConversationState;
  readonly sandboxEvents?: readonly SandboxEvent[];
  readonly historyLoading?: boolean;
  readonly historyUnavailable?: boolean;
}

/** IDs from the current exact personal access read; projections are still read afresh. */
export interface AgentLoadLineage {
  readonly agentId: string;
  readonly workspaceId: string;
  readonly sandboxId: string;
}

function withUsage(conversation: ConversationState, usage: AgentUsage): ConversationState {
  return { ...conversation, usage };
}

export function transientReadFailure(error: unknown): boolean {
  return (error instanceof ApiError && error.isRetryable) || error instanceof TransportError;
}

export async function loadAgent(
  client: CmaWebClient,
  agentId: string,
  signal: AbortSignal,
  allowIncompleteHistory = false,
  onHistoryProgress?: (ready: ReadyAgent) => void,
  lineage?: AgentLoadLineage,
): Promise<ReadyAgent & { readonly durableEvents: readonly AgentEvent[] }> {
  const v2 = await client.agentTransport?.openForLoad(agentId, { signal });
  if (v2 !== undefined)
    return loadV2Agent(
      client,
      agentId,
      signal,
      v2,
      allowIncompleteHistory,
      onHistoryProgress,
      lineage,
    );
  const [detail, transcript, usage] = await Promise.all([
    client.readAgent(agentId, { signal }),
    client.readAgentTranscript(agentId, { signal }),
    client.readAgentUsage(agentId, { signal }),
  ]);
  if (detail.id !== agentId || transcript.agentId !== agentId || usage.agentId !== agentId) {
    throw new Error('Agent projections referred to different resources.');
  }

  const workspace = await client.readWorkspace(detail.workspace.id, { signal });
  if (workspace.id !== detail.workspace.id || workspace.sandboxId !== detail.sandbox.id) {
    throw new Error('Agent projections referred to different resources.');
  }
  let conversation = conversationFromTranscript({ ...transcript, turns: [], throughSeq: 0 });
  const durableEvents: AgentEvent[] = [];
  let cursor: number | undefined = 0;
  const seen = new Set<number>([0]);
  let historyUnavailable = false;
  const snapshot = conversationFromTranscript(transcript);
  const projectedConversation = () => {
    const merged = mergeTranscriptSnapshot(conversation, snapshot, conversation.cursor, true);
    return usage.throughSeq >= (merged.usage?.throughSeq ?? 0) ? withUsage(merged, usage) : merged;
  };
  do {
    let page;
    try {
      page = await client.readAgentEvents(agentId, { cursor, signal });
    } catch (error) {
      if (signal.aborted || !allowIncompleteHistory || !transientReadFailure(error)) throw error;
      historyUnavailable = true;
      break;
    }
    signal.throwIfAborted();
    durableEvents.push(...page.items);
    conversation = page.items.reduce(foldAgentEvent, conversation);
    const nextCursor = page.next;
    if (nextCursor !== undefined && (nextCursor <= (cursor ?? 0) || seen.has(nextCursor))) {
      throw new Error('Agent event pagination did not advance.');
    }
    cursor = nextCursor;
    if (cursor !== undefined) {
      seen.add(cursor);
      // One authorized event page permits a readable snapshot, not complete-history acceptance.
      onHistoryProgress?.({
        detail,
        workspace,
        conversation: projectedConversation(),
        historyLoading: true,
        historyUnavailable: false,
      });
    }
  } while (cursor !== undefined);

  return {
    detail,
    workspace,
    conversation: projectedConversation(),
    durableEvents,
    historyLoading: false,
    historyUnavailable,
  };
}
