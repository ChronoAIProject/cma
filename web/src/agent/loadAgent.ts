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
import { loadBoundedAgent } from './loadBoundedAgent';
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
  /** A complete transcript provides the durable baseline for background live replay. */
  readonly replayReady?: boolean;
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
  onInputReady?: (ready: ReadyAgent) => void,
): Promise<ReadyAgent & { readonly durableEvents: readonly AgentEvent[] }> {
  const v2 = await client.agentTransport?.openForLoad(agentId, { signal });
  if (client.agentHistory !== undefined) {
    try {
      return await loadBoundedAgent(
        client,
        agentId,
        signal,
        v2,
        allowIncompleteHistory,
        lineage,
        onInputReady,
      );
    } catch (error) {
      if (signal.aborted || !(error instanceof ApiError) || error.status !== 404) throw error;
      // A predecessor server has no bounded endpoint. Its finite legacy log
      // remains readable; current authorization is checked again by every read.
      if (v2 !== undefined)
        return loadV2Agent(
          { ...client, agentHistory: undefined },
          agentId,
          signal,
          v2,
          allowIncompleteHistory,
          onHistoryProgress,
          lineage,
          onInputReady,
        );
      return loadAgent(
        { ...client, agentHistory: undefined, agentTransport: undefined },
        agentId,
        signal,
        allowIncompleteHistory,
        onHistoryProgress,
        lineage,
        onInputReady,
      );
    }
  }
  if (v2 !== undefined)
    return loadV2Agent(
      client,
      agentId,
      signal,
      v2,
      allowIncompleteHistory,
      onHistoryProgress,
      lineage,
      onInputReady,
    );
  const hierarchy = (async () => {
    const detailRead = client.readAgent(agentId, { signal });
    const preparedWorkspace =
      lineage?.agentId === agentId
        ? client.readWorkspace(lineage.workspaceId, { signal })
        : undefined;
    const [detail, lineageWorkspace] = await Promise.all([detailRead, preparedWorkspace]);
    signal.throwIfAborted();
    if (
      detail.id !== agentId ||
      (lineage !== undefined &&
        (lineage.agentId !== agentId ||
          lineage.workspaceId !== detail.workspace.id ||
          lineage.sandboxId !== detail.sandbox.id))
    )
      throw new Error('Agent projections referred to different resources.');
    const workspace =
      lineageWorkspace ?? (await client.readWorkspace(detail.workspace.id, { signal }));
    signal.throwIfAborted();
    if (workspace.id !== detail.workspace.id || workspace.sandboxId !== detail.sandbox.id)
      throw new Error('Agent projections referred to different resources.');
    onInputReady?.({
      detail,
      workspace,
      conversation: conversationFromTranscript({ agentId, turns: [], throughSeq: 0 }),
      historyLoading: true,
    });
    return { detail, workspace };
  })();
  const projections = Promise.all([
    hierarchy,
    client.readAgentTranscript(agentId, { signal }),
  ]).then(([metadata, transcript]) => {
    signal.throwIfAborted();
    if (transcript.agentId !== agentId)
      throw new Error('Agent projections referred to different resources.');
    onInputReady?.({
      ...metadata,
      conversation: conversationFromTranscript(transcript),
      historyLoading: true,
      historyUnavailable: false,
      replayReady: true,
    });
    return { ...metadata, transcript };
  });
  const [{ detail, workspace, transcript }, usage] = await Promise.all([
    projections,
    client.readAgentUsage(agentId, { signal }),
  ]);
  signal.throwIfAborted();
  if (transcript.agentId !== agentId || usage.agentId !== agentId)
    throw new Error('Agent projections referred to different resources.');
  let conversation = conversationFromTranscript({ ...transcript, turns: [], throughSeq: 0 });
  const durableEvents: AgentEvent[] = [];
  let cursor: number | undefined = 0;
  const seen = new Set<number>([0]);
  let historyUnavailable = false;
  const snapshot = conversationFromTranscript(transcript);
  const preview = withUsage(snapshot, usage);
  let previewPublished = onInputReady !== undefined;
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
    if (durableEvents.length > 10_128)
      throw new Error('This history requires a server with bounded history reads.');
    conversation = page.items.reduce(foldAgentEvent, conversation);
    const nextCursor = page.next;
    if (nextCursor !== undefined && (nextCursor <= (cursor ?? 0) || seen.has(nextCursor))) {
      throw new Error('Agent event pagination did not advance.');
    }
    cursor = nextCursor;
    if (cursor !== undefined) {
      seen.add(cursor);
      // Show the current transcript once, while rebuilding older rich activity
      // offscreen. A historical tool/approval must not look newly active.
      if (!previewPublished) {
        onHistoryProgress?.({
          detail,
          workspace,
          conversation: preview,
          historyLoading: true,
          historyUnavailable: false,
        });
        previewPublished = true;
      }
    }
  } while (cursor !== undefined);

  return {
    detail,
    workspace,
    conversation: historyUnavailable ? preview : projectedConversation(),
    durableEvents,
    historyLoading: false,
    historyUnavailable,
    replayReady: true,
  };
}
