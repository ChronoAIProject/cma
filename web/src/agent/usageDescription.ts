import type { AgentUsage } from '../api/types';
export function usageDescription(usage?: AgentUsage): string {
  const count = (value: number) => new Intl.NumberFormat('en').format(value);
  if (!usage) return 'Context: not reported. Total tokens: not reported.';
  const context =
    usage.contextWindow === undefined
      ? count(usage.contextUsed)
      : `${count(usage.contextUsed)} / ${count(usage.contextWindow)}`;
  const fraction =
    usage.contextWindow && usage.contextWindow > 0
      ? ` (${Math.round(Math.min(1, Math.max(0, usage.contextUsed / usage.contextWindow)) * 100)}%)`
      : '';
  const total = `${count(usage.inputTokens + usage.outputTokens)} (${count(usage.inputTokens)} input · ${count(usage.outputTokens)} output)`;
  const turn = usage.currentTurn
    ? ` This turn: ${count(usage.currentTurn.inputTokens)} input · ${count(usage.currentTurn.outputTokens)} output.`
    : '';
  return `Context: ${context}${fraction}. Total tokens: ${total}.${turn}`;
}
