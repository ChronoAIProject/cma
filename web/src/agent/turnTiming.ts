import type { ConversationState } from './model';

/** Recorded timestamps only: reconnects and approval pauses do not restart the clock. */
export function startTurn(
  state: ConversationState,
  runId: string,
  startedAt?: string,
): ConversationState {
  const previous = state.turnTiming?.[runId];
  const start = previous?.startedAt ?? startedAt;
  return {
    ...state,
    activeRunId: runId,
    activeRunStartedAt: start,
    lastTurn: { runId, outcome: 'active' },
    turnTiming:
      start === undefined || !Number.isFinite(Date.parse(start))
        ? state.turnTiming
        : { ...state.turnTiming, [runId]: { ...previous, startedAt: start } },
  };
}

export function finishTurn(
  state: ConversationState,
  runId: string | undefined,
  finishedAt?: string,
  outcome = 'unknown',
): ConversationState {
  if (runId === undefined) return state;
  const previous = state.turnTiming?.[runId];
  const startedAt =
    previous?.startedAt ?? (state.activeRunId === runId ? state.activeRunStartedAt : undefined);
  if (
    startedAt === undefined ||
    finishedAt === undefined ||
    !Number.isFinite(Date.parse(startedAt)) ||
    !Number.isFinite(Date.parse(finishedAt)) ||
    Date.parse(finishedAt) < Date.parse(startedAt)
  )
    return { ...state, lastTurn: { runId, outcome } };
  return {
    ...state,
    lastTurn: { runId, outcome },
    turnTiming: {
      ...state.turnTiming,
      [runId]: { startedAt, finishedAt: previous?.finishedAt ?? finishedAt },
    },
  };
}

export function workedFor(
  timing: NonNullable<ConversationState['turnTiming']>[string] | undefined,
) {
  if (timing?.finishedAt === undefined) return undefined;
  const elapsed = Date.parse(timing.finishedAt) - Date.parse(timing.startedAt);
  if (!Number.isFinite(elapsed) || elapsed < 0) return undefined;
  const seconds = Math.floor(elapsed / 1000);
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const part = (count: number, unit: string) => `${count} ${unit}${count === 1 ? '' : 's'}`;
  const parts = [
    ...(hours > 0 ? [part(hours, 'hour')] : []),
    ...(hours > 0 || minutes > 0 ? [part(minutes, 'minute')] : []),
    part(seconds % 60, 'second'),
  ];
  return `Worked for ${parts.join(' ')}.`;
}
