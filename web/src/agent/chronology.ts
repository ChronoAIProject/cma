import type { ConversationState } from './conversation';

export function orderEntries(
  previous: ConversationState,
  next: ConversationState,
): ConversationState {
  if (previous === next) return previous;
  const groups = ['messages', 'toolCalls', 'reasoning', 'approvals', 'recovery'] as const;
  let order = Math.max(0, ...groups.flatMap((key) => previous[key].map((item) => item.order ?? 0)));
  const result = { ...next };
  for (const key of groups) {
    const known = new Map(previous[key].map((item) => [item.id, item.order]));
    Object.assign(result, {
      [key]: next[key].map((item) => ({
        ...item,
        order: item.order ?? known.get(item.id) ?? ++order,
      })),
    });
  }
  return result;
}
