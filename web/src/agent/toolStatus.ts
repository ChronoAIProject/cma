import type { ToolCall } from './model';

const COMPLETED = ['completed', 'complete', 'success', 'succeeded'];
const FAILED = ['failed', 'error'];
const CANCELLED = ['cancelled', 'canceled', 'aborted', 'stopped', 'interrupted'];

/** Public tool states; an unfamiliar state is not proof that a call completed. */
export function isPendingTool(tool: Pick<ToolCall, 'status'>): boolean {
  const status = tool.status.toLowerCase();
  return ![...COMPLETED, ...FAILED, ...CANCELLED].includes(status);
}

/**
 * What a call's recorded status lets us claim. `unreported` is a pending call
 * whose turn has ended: the provider never said how it finished, so it is
 * neither running nor settled.
 */
export type ToolOutcome = 'running' | 'completed' | 'failed' | 'cancelled' | 'unreported';

export function toolOutcome(
  tool: Pick<ToolCall, 'status' | 'runId'>,
  activeRunId: string | undefined,
): ToolOutcome {
  const status = tool.status.toLowerCase();
  if (COMPLETED.includes(status)) return 'completed';
  if (FAILED.includes(status)) return 'failed';
  if (CANCELLED.includes(status)) return 'cancelled';
  // Only the active turn can still be running a call; a pending call from any
  // other turn was left behind when that turn ended.
  return activeRunId !== undefined && tool.runId === activeRunId ? 'running' : 'unreported';
}

/** The plain word paired with the state dot; the raw status survives only where it is evidence. */
export function toolOutcomeWord(outcome: ToolOutcome, status: string): string {
  switch (outcome) {
    case 'running':
      return 'In progress';
    case 'completed':
      return 'Completed';
    case 'failed':
      return 'Failed';
    case 'cancelled':
      return 'Cancelled';
    case 'unreported':
      return `Outcome not reported (last status: ${status})`;
  }
}
