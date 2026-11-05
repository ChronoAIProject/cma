import type { V2AgentHistoryWindow, V2AgentStatePage } from '@chronoai/cma-sdk';
import { conversationFromTranscript } from './conversation';
import type { ConversationState } from './model';
import { foldV2Item } from './transport/projection';

/** Omitted source rows are labelled, never fabricated or folded as input. */
export function conversationFromWindow(page: V2AgentHistoryWindow): ConversationState {
  const start = Number(page.start_cursor.split('.').at(-1));
  let state: ConversationState = {
    ...conversationFromTranscript({ agentId: page.agent_id, throughSeq: 0, turns: [] }),
    historyStart: start,
    historyPreviousCursor: page.previous_cursor ?? undefined,
    agentIncarnation: page.incarnation,
  };
  if (page.context !== null) state = foldV2Item(state, page.context);
  if (page.gap !== null) state = foldV2Item(state, page.gap);
  state = { ...state, cursor: start };
  state = page.items.reduce(foldV2Item, state);
  return { ...state, agentCursor: page.through_cursor };
}

export function conversationFromCurrentState(page: V2AgentStatePage): ConversationState {
  let state = conversationFromTranscript({ agentId: page.agent_id, throughSeq: 0, turns: [] });
  state = [...page.context, ...page.approvals]
    .sort((a, b) => a.sequence - b.sequence)
    .reduce(foldV2Item, state);
  return {
    ...state,
    historyStart: Number(page.through_cursor.split('.').at(-1)),
    cursor: Number(page.through_cursor.split('.').at(-1)),
    agentCursor: page.through_cursor,
    agentIncarnation: page.incarnation,
    approvalsTruncated: page.next_approval_cursor !== null,
  };
}

/** Both reads cover the same captured head; controls do not depend on the window's start. */
export function withCurrentState(
  window: ConversationState,
  page: V2AgentStatePage,
): ConversationState {
  if (window.agentCursor !== page.through_cursor || window.agentIncarnation !== page.incarnation)
    throw new Error('Agent history and current controls changed identity.');
  const current = conversationFromCurrentState(page);
  return {
    ...window,
    activeRunId: current.activeRunId,
    incompleteRuns: [
      ...new Set([...(window.incompleteRuns ?? []), ...(current.incompleteRuns ?? [])]),
    ],
    responseBinding: current.responseBinding,
    activeRunStartedAt: current.activeRunStartedAt,
    runtimeMetadata: current.runtimeMetadata,
    availableCommands: current.availableCommands,
    approvals: current.approvals,
    approvalsTruncated: current.approvalsTruncated,
    plans: current.plans.length > 0 ? current.plans : window.plans,
  };
}
