import type { ConversationState } from './model';

const groups = ['messages', 'toolCalls', 'reasoning', 'plans', 'recovery'] as const;
export const DISPLAY_ROWS = 200;
export const DISPLAY_BYTES = 4 * 1024 * 1024;
const rowSizes = new WeakMap<object, number>();
function size(value: object) {
  let bytes = rowSizes.get(value);
  if (bytes === undefined) {
    // Conservative UTF-16 storage accounting, including object field overhead.
    bytes = JSON.stringify(value).length * 2;
    rowSizes.set(value, bytes);
  }
  return bytes;
}

/** Retain a bounded display without changing the acknowledged durable cursor.
 * Pending controls have a separate bounded page and explicit navigation.
 */
export function boundConversation(state: ConversationState): ConversationState {
  const rows = groups.flatMap((group) =>
    state[group]
      .filter(
        (item) =>
          group !== 'plans' || item.runId !== state.activeRunId || state.activeRunId === undefined,
      )
      .map((item) => ({ group, item })),
  );
  let bytes = rows.reduce((total, row) => total + size(row.item), 0);
  let count = rows.length;
  let start = state.historyStart ?? 0;
  const removed = new Set<object>();
  for (const { item } of rows.sort(
    (a, b) => (a.item.order ?? a.item.sequence ?? 0) - (b.item.order ?? b.item.sequence ?? 0),
  )) {
    if (count <= DISPLAY_ROWS && bytes <= DISPLAY_BYTES) break;
    removed.add(item);
    bytes -= size(item);
    count--;
    start = Math.max(start, item.sequence ?? 0);
  }
  let next = state;
  if (removed.size > 0) {
    next = {
      ...state,
      displayLimited: true,
      historyStart: start,
      historyPreviousCursor:
        state.agentIncarnation === undefined || start === 0
          ? state.historyPreviousCursor
          : `ag1.${state.agentIncarnation}.${start}`,
    };
    for (const group of groups)
      if (state[group].some((item) => removed.has(item)))
        Object.assign(next, { [group]: state[group].filter((item) => !removed.has(item)) });
  }
  let approvals = next.approvals;
  let approvalBytes = approvals.reduce((total, item) => total + size(item), 0);
  if (approvals.length > 100 || approvalBytes > DISPLAY_BYTES) {
    approvals = approvals.slice(0, 100);
    approvalBytes = 0;
    approvals = approvals.filter((item) => (approvalBytes += size(item)) <= DISPLAY_BYTES);
    next = { ...next, approvals, approvalsTruncated: true };
  }
  const turns = new Set([next.activeRunId, next.latestTurnId]);
  for (const group of groups) for (const item of next[group]) turns.add(item.runId);
  if ((next.incompleteRuns?.length ?? 0) > DISPLAY_ROWS + 2)
    next = { ...next, incompleteRuns: next.incompleteRuns?.filter((turn) => turns.has(turn)) };
  for (const key of ['turnOutcomes', 'turnTiming', 'turnUsage'] as const) {
    const entries = Object.entries(next[key] ?? {});
    if (entries.length > DISPLAY_ROWS + 2)
      Object.assign((next = { ...next }), {
        [key]: Object.fromEntries(entries.filter(([turn]) => turns.has(turn))),
      });
  }
  if (next.visibleUsageAnchor !== undefined && removed.size > 0)
    next = {
      ...next,
      visibleUsageAnchor: {
        ...next.visibleUsageAnchor,
        parts: next.visibleUsageAnchor.parts.filter((part) =>
          next.messages.some((message) => message.id === part.id),
        ),
      },
    };
  return next;
}
