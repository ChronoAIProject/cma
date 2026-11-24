import { loadAgent } from './loadAgent';
import { mergeTranscriptSnapshot } from './chronology';
import { foldAgentEvent, type ConversationState } from './conversation';
import { foldV2Item } from './transport/projection';
import { withApprovalProjection } from './transport/externalApprovalProjection';
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

export function refreshConversation(
  current: ConversationState,
  next: Awaited<ReturnType<typeof loadAgent>>,
): ConversationState {
  if (next.v2Items !== undefined) {
    if (current.agentIncarnation !== next.conversation.agentIncarnation)
      throw new Error('Agent snapshot changed conversation identity.');
    // Policy belongs to the freshly qualified read, even when its durable cursor
    // is unchanged. Preserve newer live/terminal state while replacing authority.
    const baseline = withApprovalProjection(
      current,
      next.conversation.externalApprovalProjection ?? { supported: false, canReview: false },
    );
    return next.v2Items.reduce(foldV2Item, baseline);
  }
  if (current.cursor > next.conversation.cursor) return current;
  const folded = next.durableEvents.reduce(foldAgentEvent, current);
  return {
    ...mergeTranscriptSnapshot(folded, next.conversation, current.cursor),
    usage:
      (next.conversation.usage?.throughSeq ?? 0) >= (folded.usage?.throughSeq ?? 0)
        ? next.conversation.usage
        : folded.usage,
  };
}
