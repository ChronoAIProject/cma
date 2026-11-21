import type { AgentUsage } from '../api/types';

const format = new Intl.NumberFormat('en');
const unreported = 'not reported';

export const usageDefinitions = {
  context: 'Latest provider observation for one context window.',
  total: 'Provider-reported input and output accumulated for this Agent.',
  turn: 'Provider-reported usage attributed to this CMA turn.',
  billing: 'Input includes any cache reads. Cache breakdown and billed usage are unavailable here.',
};

export function contextFraction(usage: AgentUsage | undefined): number | undefined {
  const limit = usage?.contextWindow;
  if (usage === undefined || limit === undefined || limit <= 0) return undefined;
  return Math.min(1, Math.max(0, usage.contextUsed / limit));
}

function tokenSplit({
  inputTokens,
  outputTokens,
}: Pick<AgentUsage, 'inputTokens' | 'outputTokens'>) {
  return `${format.format(inputTokens)} input · ${format.format(outputTokens)} output`;
}

export function usageSummary(usage?: AgentUsage) {
  if (usage === undefined) return { context: unreported, total: unreported };

  const fraction = contextFraction(usage);
  const used = format.format(usage.contextUsed);
  const context =
    fraction === undefined || usage.contextWindow === undefined
      ? used
      : `${used} / ${format.format(usage.contextWindow)} (${Math.round(fraction * 100)}%)`;
  return {
    context,
    total: format.format(usage.inputTokens + usage.outputTokens),
    split: tokenSplit(usage),
    turn: usage.currentTurn && tokenSplit(usage.currentTurn),
  };
}

export function usageDescription(usage?: AgentUsage): string {
  const { context, total, split, turn } = usageSummary(usage);
  return [
    `Context: ${context}. ${usageDefinitions.context}`,
    `Total tokens: ${total}${split === undefined ? '' : ` (${split})`}. ${usageDefinitions.total}`,
    turn === undefined ? '' : `This turn: ${turn}. ${usageDefinitions.turn}`,
    usageDefinitions.billing,
  ]
    .filter(Boolean)
    .join(' ');
}
