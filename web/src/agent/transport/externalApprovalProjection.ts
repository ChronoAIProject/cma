import { ProtocolError } from '@cma/client';
import type {
  ExternalApprovalKind,
  ExternalApprovalProjection,
} from '../../api-client/agent-events-v2/external-approval';
import type { ConversationState } from '../model';
import type { ExternalApprovalRequest } from '../externalApproval';
import { orderEntries } from '../chronology';

export function sameApprovalProjection(
  left: ExternalApprovalProjection | undefined,
  right: ExternalApprovalProjection,
) {
  return (
    left?.supported === right.supported &&
    left.canReview === right.canReview &&
    left.reviewOrigin === right.reviewOrigin
  );
}

export function withApprovalProjection(
  state: ConversationState,
  projection: ExternalApprovalProjection,
): ConversationState {
  return {
    ...state,
    externalApprovalProjection: projection,
    externalApprovalReviewPolicy:
      projection.supported && projection.canReview && projection.reviewOrigin
        ? { origin: projection.reviewOrigin, path: '/assistant/approvals' }
        : undefined,
  };
}

/** The rich decoder establishes Agent/incarnation and sequence before this fold. */
export function foldExternalApproval(
  state: ConversationState,
  kind: ExternalApprovalKind,
  sequence: number,
): ConversationState {
  if (!state.externalApprovalProjection?.supported || !state.agentIncarnation)
    throw new ProtocolError('External approval was not negotiated for this conversation');
  const runId = String(kind.turn_sequence);
  if (state.turnOutcomes?.[runId] !== undefined) return state;
  const id = `external:${state.agentIncarnation}:${runId}:${kind.approval_id}`;
  const found = state.approvals.find((approval) => approval.id === id);
  if (found && found.source !== 'external')
    throw new ProtocolError('Approval identity conflicts with native approval');
  const previous = found as ExternalApprovalRequest | undefined;
  if (previous && kind.revision < previous.revision) return state;
  if (kind.type === 'external_approval_requested') {
    if (previous?.decision !== undefined) {
      if (kind.revision === previous.revision)
        throw new ProtocolError('External approval revision has conflicting content');
      return state;
    }
    if (state.activeRunId !== runId) return state;
    const binding = state.responseBinding;
    if (binding?.turnSequence !== kind.turn_sequence)
      throw new ProtocolError('External approval has no exact original turn binding');
    if (previous) {
      if (
        previous.waitingSince !== kind.waiting_since ||
        Date.parse(kind.deadline_at) > Date.parse(previous.deadlineAt)
      )
        throw new ProtocolError('External approval attempted to reset its waiting budget');
      if (kind.revision === previous.revision) {
        if (
          kind.deadline_at !== previous.deadlineAt ||
          (kind.approve_url ?? undefined) !== previous.reviewUrl
        )
          throw new ProtocolError('External approval revision has conflicting content');
        return state;
      }
    }
    const approval: ExternalApprovalRequest = {
      ...previous,
      source: 'external',
      provider: kind.provider,
      id,
      runId,
      operationId: kind.approval_id,
      revision: kind.revision,
      waitingSince: kind.waiting_since,
      deadlineAt: kind.deadline_at,
      reviewUrl: kind.approve_url ?? undefined,
      canReview: kind.approve_url !== null,
      reason: 'Model request approval',
      sequence: previous?.sequence ?? sequence,
    };
    return orderEntries(
      state,
      { ...state, approvals: [...state.approvals.filter((item) => item.id !== id), approval] },
      sequence,
    );
  }
  if (!previous) throw new ProtocolError('External approval resolution has no retained request');
  if (kind.revision === previous.revision) {
    if (kind.resolution !== previous.decision)
      throw new ProtocolError('External approval revision has conflicting content');
    return state;
  }
  if (previous.decision !== undefined) {
    if (kind.resolution !== previous.decision)
      throw new ProtocolError('External approval resolution conflicts with its recorded result');
    return state;
  }
  return {
    ...state,
    approvals: state.approvals.map((item) =>
      item === previous
        ? { ...previous, revision: kind.revision, decision: kind.resolution }
        : item,
    ),
  };
}
