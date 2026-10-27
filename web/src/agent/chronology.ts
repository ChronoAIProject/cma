import type { ConversationState } from './model';
import { isPendingTool } from './toolStatus';

const groups = ['messages', 'toolCalls', 'reasoning', 'approvals', 'plans', 'recovery'] as const;

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
  let order = Math.max(0, ...groups.flatMap((key) => previous[key].map((item) => item.order ?? 0)));
  const result = { ...next };
  for (const key of groups) {
    const known = new Map(previous[key].map((item) => [item.id, item]));
    Object.assign(result, {
      [key]: next[key].map((item) => {
        const previousItem = known.get(item.id);
        const settled =
          key === 'toolCalls' &&
          'status' in item &&
          previousItem !== undefined &&
          'status' in previousItem &&
          isPendingTool(previousItem) &&
          !isPendingTool(item);
        return {
          ...item,
          sequence: settled
            ? sequence
            : (item.sequence ??
              known.get(item.id)?.sequence ??
              (item === known.get(item.id) ? undefined : sequence)),
          order: settled ? ++order : (item.order ?? known.get(item.id)?.order ?? ++order),
        };
      }),
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
    } else {
      missing.push(row);
      rowById.set(`messages:${message.id}`, row);
    }
    messages.set(message.id, message);
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
  const result = { ...current, messages: [...messages.values()], recovery };
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
