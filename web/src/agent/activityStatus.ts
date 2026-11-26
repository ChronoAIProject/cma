import type { AgentSummary } from '../api/types';
import type { ReadyAgent } from './loadAgent';
import { currentAgentSnapshot, currentAgentStatus } from './status';
import { currentExternalApproval } from './externalApproval';

export const activityWords = {
  loading: 'Loading activity',
  unknown: 'Status unavailable',
  working: 'Working',
  waiting_approval: 'Waiting for approval',
  completed: 'Ready',
  failed: 'Failed',
  interrupted: 'Response interrupted',
} as const;
export type ActivityStatus = keyof typeof activityWords;

/** Display evidence only; resource status still owns command eligibility. */
export function observedActivity(
  state: ReadyAgent,
  summary?: AgentSummary,
): ActivityStatus | undefined {
  const { detail, conversation } = state;
  const status = currentAgentStatus(detail, conversation, summary);
  if (status !== 'ready' && status !== 'working' && status !== 'waiting_approval') return undefined;
  if (state.historyLoading) return 'loading';
  if (state.historyUnavailable) return 'unknown';
  if (currentExternalApproval(conversation)) return 'waiting_approval';
  const runId = conversation.activeRunId ?? conversation.latestTurnId;
  if (runId === undefined) return undefined;
  const timing = conversation.turnTiming?.[runId];
  const active = conversation.activeRunId !== undefined;
  const at = active ? timing?.startedAt : timing?.finishedAt;
  if (!(Date.parse(at ?? '') > Date.parse(currentAgentSnapshot(detail, summary).updatedAt)))
    return undefined;
  if (active) return status === 'waiting_approval' ? 'waiting_approval' : 'working';
  return conversation.turnOutcomes?.[runId];
}
