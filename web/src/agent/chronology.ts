import type { ConversationState } from './model';
import { isPendingTool } from './toolStatus';

const groups = ['messages', 'toolCalls', 'reasoning', 'approvals', 'plans', 'recovery'] as const;

const maximumOrders = new WeakMap<readonly { readonly order?: number }[], number>();
const orderedGroups = new WeakSet<readonly { readonly order?: number }[]>();

function maximumOrder(state: ConversationState) {
  let maximum = 0;
  for (const group of groups) {
    const values = state[group];
    let cached = maximumOrders.get(values);
    if (cached === undefined) {
      cached = 0;
      for (const item of values) cached = Math.max(cached, item.order ?? 0);
      maximumOrders.set(values, cached);
    }
    maximum = Math.max(maximum, cached);
  }
  return maximum;
}

interface Row {
  group: (typeof groups)[number];
  id: string;
  sequence?: number;
  afterSequence?: number;
  runId?: string;
  order: number;
}

function insertMissingRows(rows: Row[], missing: Row[]): Row[] {
  const anchors = rows
    .filter((row) => row.sequence !== undefined)
    .sort((left, right) => left.sequence! - right.sequence!);
  const positions = new Map(rows.map((row, index) => [row, index]));
  const buckets = new Map<number, Row[]>();
  const position = (row: Row) =>
    row.sequence ?? (row.afterSequence === undefined ? Infinity : row.afterSequence + 0.5);
  for (const row of missing.sort((left, right) => position(left) - position(right))) {
    const sequence = position(row);
    let low = 0;
    let high = anchors.length;
    while (low < high) {
      const middle = Math.floor((low + high) / 2);
      if (anchors[middle]!.sequence! < sequence) low = middle + 1;
      else high = middle;
    }
    const lowerAt = low === 0 ? -1 : positions.get(anchors[low - 1]!)!;
    const upperAt = low === anchors.length ? rows.length : positions.get(anchors[low]!)!;
    const runEnd =
      row.group === 'recovery' && row.runId !== undefined
        ? rows.reduce((last, entry, index) => (entry.runId === row.runId ? index : last), -1)
        : -1;
    const at = Math.max(lowerAt + 1, upperAt, runEnd + 1);
    const bucket = buckets.get(at) ?? [];
    bucket.push(row);
    buckets.set(at, bucket);
  }
  return [
    ...rows.flatMap((row, index) => [...(buckets.get(index) ?? []), row]),
    ...(buckets.get(rows.length) ?? []),
  ];
}

export function orderEntries(
  previous: ConversationState,
  next: ConversationState,
  sequence?: number,
): ConversationState {
  if (previous === next) return previous;
  let order = maximumOrder(previous);
  const nextOrder = () => ++order;
  const result = { ...next };
  for (const key of groups) {
    const before = previous[key];
    const after = next[key];
    if (before === after && orderedGroups.has(after)) continue;
    let known: Map<string, (typeof before)[number]> | undefined;
    let groupMaximum = 0;
    let unchanged = true;
    const ordered = after.map((item, index) => {
      const previousItem =
        before[index]?.id === item.id
          ? before[index]
          : (known ??= new Map(before.map((entry) => [entry.id, entry]))).get(item.id);
      if (item === previousItem && item.order !== undefined) {
        groupMaximum = Math.max(groupMaximum, item.order);
        return item;
      }
      const settled =
        key === 'toolCalls' &&
        'status' in item &&
        previousItem !== undefined &&
        'status' in previousItem &&
        isPendingTool(previousItem) &&
        !isPendingTool(item);
      const itemSequence = settled
        ? sequence
        : (item.sequence ??
          previousItem?.sequence ??
          (item === previousItem ? undefined : sequence));
      const itemOrder = settled ? nextOrder() : (item.order ?? previousItem?.order ?? nextOrder());
      groupMaximum = Math.max(groupMaximum, itemOrder);
      if (itemSequence === item.sequence && itemOrder === item.order) return item;
      unchanged = false;
      return { ...item, sequence: itemSequence, order: itemOrder };
    });
    const values = unchanged ? after : ordered;
    orderedGroups.add(values);
    maximumOrders.set(values, groupMaximum);
    Object.assign(result, {
      [key]: values,
    });
  }
  return result;
}

/** Sequence anchors place missing durable rows without treating live display order as a cursor. */
export function mergeTranscriptSnapshot(
  current: ConversationState,
  snapshot: ConversationState,
  previousCursor = current.cursor,
  trustEqualState = false,
): ConversationState {
  const rows: Row[] = groups
    .flatMap((group) =>
      current[group].map((item) => ({
        group,
        id: item.id,
        sequence: item.sequence,
        runId: 'runId' in item ? item.runId : undefined,
        order: item.order ?? 0,
      })),
    )
    .sort((left, right) => left.order - right.order);
  const rowById = new Map(rows.map((row) => [`${row.group}:${row.id}`, row]));
  const messages = new Map(current.messages.map((message) => [message.id, message]));
  const liveByRun = new Map<string | undefined, (typeof current.messages)[number]>();
  const replacedLiveRuns = new Set<string>();
  for (const message of current.messages)
    if (message.liveOnly && !liveByRun.has(message.runId)) liveByRun.set(message.runId, message);
  const missing: Row[] = [];
  for (const message of snapshot.messages) {
    const sequence = message.sequence ?? Number(message.id.slice('event:'.length));
    const alias = messages.get(`msg_${String(sequence)}`);
    const existing = messages.get(message.id) ?? (alias?.liveOnly ? undefined : alias);
    const row = {
      group: 'messages' as const,
      id: message.id,
      sequence,
      runId: message.runId,
      order: 0,
    };
    if (existing !== undefined) {
      const known = rowById.get(`messages:${existing.id}`)!;
      Object.assign(known, row);
      rowById.set(`messages:${message.id}`, known);
      messages.delete(existing.id);
      messages.set(message.id, {
        ...message,
        renderKey: message.renderKey ?? existing.renderKey ?? existing.id,
        commandId: message.commandId ?? existing.commandId,
        status:
          current.cursor >= snapshot.cursor && existing.status === 'error'
            ? 'error'
            : message.status,
      });
      continue;
    }
    const live =
      message.role === 'assistant' && sequence > previousCursor
        ? liveByRun.get(message.runId)
        : undefined;
    if (live !== undefined) {
      messages.delete(live.id);
      liveByRun.delete(message.runId);
      const known = rowById.get(`messages:${live.id}`)!;
      Object.assign(known, row);
      rowById.set(`messages:${message.id}`, known);
      if (message.runId !== undefined) replacedLiveRuns.add(message.runId);
    } else {
      missing.push(row);
      rowById.set(`messages:${message.id}`, row);
    }
    messages.set(
      message.id,
      live === undefined
        ? message
        : { ...message, renderKey: message.renderKey ?? live.renderKey ?? live.id },
    );
  }
  const recovery = [...current.recovery];
  for (const notice of snapshot.recovery) {
    if (
      recovery.some(
        (entry) =>
          entry.id === notice.id ||
          (notice.runId !== undefined &&
            entry.runId === notice.runId &&
            entry.title === notice.title),
      )
    )
      continue;
    recovery.push(notice);
    missing.push({
      group: 'recovery',
      id: notice.id,
      sequence: notice.sequence,
      afterSequence: notice.afterSequence,
      runId: notice.runId,
      order: 0,
    });
  }
  const completedRuns = new Set(replacedLiveRuns);
  if (
    snapshot.cursor > previousCursor &&
    current.activeRunId !== undefined &&
    snapshot.activeRunId !== current.activeRunId
  )
    completedRuns.add(current.activeRunId);
  const clearedRecovery = recovery.filter(
    (notice) => notice.clearsOn === undefined || !completedRuns.has(notice.runId ?? ''),
  );
  const clearedGapRuns =
    current.liveGapRuns === undefined
      ? undefined
      : new Set([...current.liveGapRuns].filter((runId) => !completedRuns.has(runId)));
  const result = {
    ...current,
    messages: [...messages.values()],
    recovery: clearedRecovery,
    liveGapRuns: clearedGapRuns?.size === 0 ? undefined : clearedGapRuns,
  };
  const positions = new Map(
    insertMissingRows(rows, missing).map((row, index) => [`${row.group}:${row.id}`, index + 1]),
  );
  for (const group of groups)
    Object.assign(result, {
      [group]: result[group]
        .map((item) => ({
          ...item,
          order: positions.get(`${group}:${item.id}`)!,
        }))
        .sort((left, right) => left.order - right.order),
    });
  const useSnapshotState =
    snapshot.cursor > previousCursor || (trustEqualState && snapshot.cursor === previousCursor);
  const activeRunId = useSnapshotState ? snapshot.activeRunId : current.activeRunId;
  return {
    ...result,
    title: useSnapshotState ? (snapshot.title ?? current.title) : (current.title ?? snapshot.title),
    ownerTitle: useSnapshotState
      ? (snapshot.ownerTitle ?? current.ownerTitle)
      : (current.ownerTitle ?? snapshot.ownerTitle),
    latestTurnId:
      Number(current.latestTurnId ?? 0) >= Number(snapshot.latestTurnId ?? 0)
        ? current.latestTurnId
        : snapshot.latestTurnId,
    turnOutcomes: useSnapshotState
      ? { ...current.turnOutcomes, ...snapshot.turnOutcomes }
      : { ...snapshot.turnOutcomes, ...current.turnOutcomes },
    turnTiming: Object.fromEntries(
      [
        ...new Set([
          ...Object.keys(snapshot.turnTiming ?? {}),
          ...Object.keys(current.turnTiming ?? {}),
        ]),
      ].map((runId) => [
        runId,
        {
          ...(current.turnTiming?.[runId] ?? snapshot.turnTiming![runId]!),
          finishedAt:
            current.turnTiming?.[runId]?.finishedAt ?? snapshot.turnTiming?.[runId]?.finishedAt,
        },
      ]),
    ),
    activeRunId,
    activeRunStartedAt:
      activeRunId === undefined
        ? undefined
        : useSnapshotState
          ? snapshot.activeRunStartedAt
          : (current.activeRunStartedAt ??
            (snapshot.activeRunId === activeRunId ? snapshot.activeRunStartedAt : undefined)),
    reasoning: result.reasoning.filter(
      (block) => activeRunId !== undefined && block.runId === activeRunId,
    ),
    cursor: Math.max(current.cursor, snapshot.cursor),
  };
}
