import type { ConversationState } from './model';
import { visibleTurnText } from './tokenEstimate';
function number(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined;
}

export function durableUsage(state: ConversationState, kind: Readonly<Record<string, unknown>>) {
  const inputTokens = number(kind.input_tokens);
  const outputTokens = number(kind.output_tokens);
  const contextUsed = number(kind.context_used);
  if (inputTokens === undefined || outputTokens === undefined || contextUsed === undefined) {
    return state;
  }
  const turnSequence = number(kind.turn_sequence);
  const currentTurn =
    turnSequence === undefined
      ? state.usage?.currentTurn
      : {
          turnSequence,
          inputTokens: number(kind.turn_input_tokens) ?? 0,
          outputTokens: number(kind.turn_output_tokens) ?? 0,
        };
  return {
    ...state,
    visibleUsageAnchor:
      currentTurn !== undefined &&
      String(currentTurn.turnSequence) === state.activeRunId &&
      (state.usage?.currentTurn?.turnSequence !== currentTurn.turnSequence ||
        state.usage.currentTurn.inputTokens !== currentTurn.inputTokens ||
        state.usage.currentTurn.outputTokens !== currentTurn.outputTokens)
        ? { ...currentTurn, parts: visibleTurnText(state) }
        : state.visibleUsageAnchor,
    turnUsage:
      currentTurn === undefined
        ? state.turnUsage
        : { ...state.turnUsage, [currentTurn.turnSequence]: currentTurn },
    usage: {
      agentId: state.usage?.agentId ?? '',
      throughSeq: state.cursor,
      inputTokens,
      outputTokens,
      contextUsed,
      contextWindow: number(kind.context_window),
      currentTurn,
    },
  };
}
