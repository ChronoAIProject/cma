import type { ToolCall } from './model';

/** Public tool states; an unfamiliar state is not proof that a call completed. */
export function isPendingTool(tool: Pick<ToolCall, 'status'>): boolean {
  return ![
    'completed',
    'complete',
    'failed',
    'error',
    'cancelled',
    'canceled',
    'aborted',
    'stopped',
    'interrupted',
    'success',
    'succeeded',
  ].includes(tool.status.toLowerCase());
}
