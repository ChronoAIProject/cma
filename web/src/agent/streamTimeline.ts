import type { ConversationState } from './model';
import { groupToolCalls, type Entry, type TimelineItem } from './timeline';
import { isPendingTool, toolOutcome } from './toolStatus';

const entryCache = new WeakMap<Entry['item'], Entry>();

function entry<Kind extends Entry['kind']>(
  kind: Kind,
  item: Extract<Entry, { kind: Kind }>['item'],
): Entry {
  const cached = entryCache.get(item);
  if (cached?.kind === kind) return cached;
  const value = { kind, item } as Entry;
  entryCache.set(item, value);
  return value;
}

function samePosition(previous: Entry, next: Entry) {
  return (
    previous.kind === next.kind &&
    previous.item.id === next.item.id &&
    previous.item.order === next.item.order &&
    (previous.kind !== 'message' ||
      next.kind !== 'message' ||
      (previous.item.role === next.item.role &&
        previous.item.runId === next.item.runId &&
        previous.item.liveOnly === next.item.liveOnly))
  );
}

function layout(entries: readonly Entry[]) {
  const ordered = [...entries].sort(
    (left, right) => (left.item.order ?? 0) - (right.item.order ?? 0),
  );
  const finalReply = new Map<string, string>();
  const continued = new Set<string>();
  let previousRole: 'user' | 'assistant' | undefined;
  for (const current of ordered) {
    if (current.kind !== 'message') continue;
    const message = current.item;
    if (message.role === 'assistant' && message.runId !== undefined && !message.liveOnly)
      finalReply.set(message.runId, message.id);
    if (message.role === previousRole) continued.add(message.id);
    previousRole = message.role;
  }
  const items = groupToolCalls(ordered);
  const positions = new Map<string, number>();
  items.forEach((item, index) => {
    if (item.kind === 'tools')
      for (const tool of item.items) positions.set(`tool:${tool.id}`, index);
    else positions.set(`${item.kind}:${item.item.id}`, index);
  });
  return { items, positions, finalReply, continued, previousRole };
}

export function createStreamTimeline() {
  let previous:
    | {
        conversation: ConversationState;
        entries: readonly Entry[];
        projection: ReturnType<typeof layout>;
      }
    | undefined;
  return (conversation: ConversationState) => {
    const before = previous?.conversation;
    if (
      before !== undefined &&
      before.messages === conversation.messages &&
      before.toolCalls === conversation.toolCalls &&
      before.approvals === conversation.approvals &&
      before.plans === conversation.plans &&
      before.recovery === conversation.recovery &&
      before.activeRunId === conversation.activeRunId
    )
      return previous!.projection;
    const entries: Entry[] = [
      ...conversation.messages.map((item) => entry('message', item)),
      ...conversation.toolCalls
        .filter(
          (item) =>
            !isPendingTool(item) ||
            item.runId !== conversation.activeRunId ||
            conversation.activeRunId === undefined,
        )
        .map((item) => entry('tool', item)),
      ...conversation.approvals.map((item) => entry('approval', item)),
      ...conversation.plans
        .filter((item) => item.entries.length > 0 || item.incomplete)
        .map((item) => entry('plan', item)),
      ...conversation.recovery.map((item) => entry('recovery', item)),
    ];
    let projection: ReturnType<typeof layout>;
    if (
      previous === undefined ||
      previous.entries.length !== entries.length ||
      entries.some((item, index) => !samePosition(previous!.entries[index]!, item))
    ) {
      projection = layout(entries);
    } else {
      projection = previous.projection;
      let items: TimelineItem[] | undefined;
      entries.forEach((item, index) => {
        if (item === previous!.entries[index]) return;
        items ??= [...projection.items];
        const position = projection.positions.get(`${item.kind}:${item.item.id}`)!;
        const current = items[position]!;
        if (current.kind === 'tools' && item.kind === 'tool') {
          const tools = current.items.map((tool) => (tool.id === item.item.id ? item.item : tool));
          items[position] = {
            ...current,
            items: tools,
            failed: tools.some((tool) => toolOutcome(tool, undefined) === 'failed'),
          };
        } else items[position] = item;
      });
      if (items !== undefined) projection = { ...projection, items };
    }
    previous = { conversation, entries, projection };
    return projection;
  };
}
