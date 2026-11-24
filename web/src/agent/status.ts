import type { AgentDetail, AgentStatus, AgentSummary } from '../api/types';
import type { ConversationState } from './model';

export function sameAgent(detail: AgentDetail, summary: AgentSummary): boolean {
  return (
    detail.id === summary.id &&
    detail.sandbox.id === summary.sandboxId &&
    detail.workspace.id === summary.workspaceId &&
    detail.worktree.id === summary.worktreeId
  );
}

export function currentAgentSnapshot(
  detail: AgentDetail,
  summary?: AgentSummary,
): AgentDetail | AgentSummary {
  return summary !== undefined &&
    sameAgent(detail, summary) &&
    Date.parse(summary.updatedAt) > Date.parse(detail.updatedAt)
    ? summary
    : detail;
}

export function currentAgentStatus(
  detail: AgentDetail,
  conversation: ConversationState,
  summary?: AgentSummary,
): AgentStatus {
  const snapshot = currentAgentSnapshot(detail, summary);
  // Historical status frames must not override a newer resource snapshot.
  return conversation.agentStatus !== undefined &&
    conversation.agentStatusAt !== undefined &&
    Date.parse(conversation.agentStatusAt) > Date.parse(snapshot.updatedAt)
    ? conversation.agentStatus
    : snapshot.status;
}
