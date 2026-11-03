import { ApiError, ProtocolError } from '@chronoai/cma-sdk';
import { ReplayPositionError } from './replayIndex';

export function replayError(cause: unknown): string {
  if (cause instanceof ReplayPositionError) return cause.message;
  if (cause instanceof ApiError) {
    if ([401, 403, 404].includes(cause.status))
      return 'Operation history access is no longer available.';
    if (cause.status === 410)
      return 'Operation history expired. The selected record is no longer retained.';
    if (cause.status === 409)
      return 'Saved replay position expired because the Agent incarnation changed.';
    if (cause.code === 'agent_history_unavailable')
      return 'History gap: the requested recorded history cannot be verified.';
  }
  if (cause instanceof ProtocolError)
    return 'History gap: event order or identity could not be verified. No partial history is displayed.';
  return 'Operation history is temporarily unavailable. Retry the same position or Return Live.';
}
