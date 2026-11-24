import { ApprovalBaselineRequired } from './approvalBaseline';
import type { AgentResourceState } from '../controllerTypes';
import type { AgentV2Session } from './client';
import type { ExternalApprovalProjection } from '../../api-client/agent-events-v2/external-approval';
import { sameApprovalProjection, withApprovalProjection } from './externalApprovalProjection';

const unavailable = { supported: false, canReview: false };

/** A late history response cannot restore action authority after renegotiation failed. */
export function approvalProjectionView(state: AgentResourceState): AgentResourceState {
  return state.status === 'ready' && state.v2?.approvalProjectionAvailable === false
    ? { ...state, conversation: withApprovalProjection(state.conversation, unavailable) }
    : state;
}

export function observeApprovalProjection(
  session: AgentV2Session,
  agentId: string,
  signal: AbortSignal,
  state: { readonly current: AgentResourceState },
  publish: (state: AgentResourceState) => void,
) {
  return (projection: ExternalApprovalProjection) => {
    const current = state.current;
    if (
      signal.aborted ||
      current.status !== 'ready' ||
      current.detail.id !== agentId ||
      current.v2 !== session
    )
      return;
    const previous = current.conversation.externalApprovalProjection;
    if (
      !session.approvalProjectionAvailable ||
      (previous !== undefined && !sameApprovalProjection(previous, projection))
    ) {
      session.invalidateApprovalProjection();
      publish({
        ...current,
        conversation: withApprovalProjection(current.conversation, unavailable),
      });
      throw new ApprovalBaselineRequired('policy');
    }
    publish({ ...current, conversation: withApprovalProjection(current.conversation, projection) });
  };
}
