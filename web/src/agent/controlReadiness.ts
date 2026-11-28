import type { AgentResourceState } from './controllerTypes';

// Roll back the v2 early-control boundary without changing history hydration.
const LIVE_HEAD_CONTROLS_ENABLED = true;

export function agentRestartReady(state: AgentResourceState, agentId: string | undefined): boolean {
  return (
    state.status === 'ready' &&
    state.detail.id === agentId &&
    state.detail.status === 'stopped' &&
    state.detail.restartAvailable === true
  );
}

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

export type ResponseControlAvailability =
  | 'available'
  | 'checking'
  | 'checking-access'
  | 'check-failed'
  | 'inactive'
  | 'unsupported'
  | 'unbound'
  | 'denied';

export function controlsUnavailable(
  state: AgentResourceState,
  hasV2Session: boolean,
  suspended: boolean,
): boolean {
  return (
    suspended ||
    (hasV2Session && state.status === 'ready' && state.conversation.responseBinding === undefined)
  );
}

export function responseControlAvailability(
  state: AgentResourceState,
  agentId: string | undefined,
  suspended = false,
): ResponseControlAvailability {
  if (suspended) return 'checking-access';
  if (!controlBaselineReady(state, agentId) || state.status !== 'ready') return 'checking';
  if (state.v2 !== undefined && state.conversation.responseBinding === undefined)
    return state.historyLoading || state.historyUnavailable ? 'checking' : 'unbound';
  return 'available';
}
