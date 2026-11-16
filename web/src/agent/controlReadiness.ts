import type { AgentResourceState } from './controllerTypes';

// Roll back the v2 early-control boundary without changing history hydration.
const LIVE_HEAD_CONTROLS_ENABLED = true;

/** A verified live head supplies control targets independently of old activity. */
export function controlBaselineReady(
  state: AgentResourceState,
  agentId: string | undefined,
  suspended = false,
): boolean {
  if (suspended || state.status !== 'ready' || state.detail.id !== agentId) return false;
  if (LIVE_HEAD_CONTROLS_ENABLED && state.v2 !== undefined)
    return state.conversation.agentCursor !== undefined;
  return !state.historyLoading && !state.historyUnavailable;
}
