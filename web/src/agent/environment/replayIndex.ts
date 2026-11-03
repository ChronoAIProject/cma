import { ProtocolError, type V2AgentEventItem } from '@chronoai/cma-sdk';
import type { CmaWebClient } from '../../api/client';
import type { PlanEntry } from '../model';

export interface OperationRecord {
  readonly id: string;
  readonly turn: string;
  readonly cursor: string;
  readonly startedCursor: string;
  readonly title: string;
  readonly status: string;
  readonly at: string;
  readonly output?: { readonly text: string; readonly truncated: boolean };
  readonly plan?: readonly PlanEntry[];
}

export interface ReplayWindow {
  readonly records: readonly OperationRecord[];
  readonly through: string;
  readonly older?: string;
  readonly newer: boolean;
  readonly gap: boolean;
  readonly turn?: string;
  readonly plan?: readonly PlanEntry[];
}

export type ReplayReader = NonNullable<CmaWebClient['operationReplay']>;
export interface ReplayQuery {
  readonly through?: string;
  readonly after?: string;
  readonly turn?: string;
}

const sequence = (cursor: string) => Number(cursor.split('.')[2]);

export class ReplayPositionError extends Error {}

function replayIndex(query: ReplayQuery, seed?: ReplayWindow) {
  const records = new Map<string, OperationRecord>(
    seed?.records.map((record) => [record.id, record]),
  );
  let turn = seed?.turn;
  let plan = seed?.plan;
  let older = seed?.older;
  let newer = false;
  let gap = seed?.gap ?? false;
  const accept = (item: V2AgentEventItem) => {
    const kind = item.kind;
    if (kind?.type === 'turn_started') {
      turn = String(kind.turn_sequence);
      plan = undefined;
    } else if (kind?.type === 'history_gap') {
      gap = true;
      plan = undefined;
    } else if (kind?.type === 'plan_set') {
      plan = kind.entries;
    } else if (turn !== undefined && (query.turn === undefined || query.turn === turn)) {
      if (kind?.type === 'tool_started' && kind.kind === 'execute') {
        if (query.after && item.sequence <= sequence(query.after)) return;
        if (query.after && records.size === 50) {
          newer = true;
          return;
        }
        const id = `${turn}:${kind.call_id}`;
        records.set(id, {
          id,
          turn,
          cursor: item.cursor,
          startedCursor: item.cursor,
          title: kind.title,
          status: kind.status,
          at: item.appended_at,
          plan,
        });
      } else if (kind?.type === 'tool_updated' || kind?.type === 'tool_output_reported') {
        const id = `${turn}:${kind.call_id}`;
        const previous = records.get(id);
        if (!previous) return;
        records.delete(id);
        records.set(id, {
          ...previous,
          cursor: item.cursor,
          at: item.appended_at,
          plan,
          ...(kind.type === 'tool_updated'
            ? { title: kind.title ?? previous.title, status: kind.status }
            : {}),
          output: kind.output ?? previous.output,
        });
      }
      if (records.size > 50) {
        const first = records.values().next().value!;
        older = first.cursor;
        records.delete(first.id);
      }
    }
  };
  return {
    accept,
    snapshot: (through: string): ReplayWindow => ({
      records: [...records.values()],
      through,
      older,
      newer,
      gap,
      turn,
      plan,
    }),
  };
}

export async function readReplayWindow(
  reader: ReplayReader,
  agentId: string,
  signal: AbortSignal,
  query: ReplayQuery = {},
): Promise<ReplayWindow> {
  const index = replayIndex(query);
  let pageCursor: string | undefined;
  let through = '';
  let found = query.through === undefined;
  do {
    const page = await reader.snapshot(agentId, {
      signal,
      limit: 200,
      pageCursor,
      toolOutput: true,
    });
    signal.throwIfAborted();
    if (query.through && query.through.split('.')[1] !== page.incarnation)
      throw new ReplayPositionError(
        'Saved replay position expired because the Agent incarnation changed.',
      );
    if (query.after && query.after.split('.')[1] !== page.incarnation)
      throw new ReplayPositionError(
        'Saved replay position expired because the Agent incarnation changed.',
      );
    through = page.through_cursor;
    for (const item of page.items) {
      if (query.through && item.sequence > sequence(query.through)) break;
      index.accept(item);
      if (item.cursor === query.through) {
        found = true;
        through = item.cursor;
        break;
      }
    }
    if (query.through && found) break;
    pageCursor = page.next_page_cursor ?? undefined;
  } while (pageCursor);
  if (!found) throw new ReplayPositionError('Saved replay position is missing or expired.');
  return index.snapshot(through);
}

export async function advanceReplayWindow(
  reader: ReplayReader,
  agentId: string,
  signal: AbortSignal,
  previous: ReplayWindow,
): Promise<ReplayWindow> {
  const index = replayIndex({}, previous);
  let through = previous.through;
  await reader.events(agentId, {
    signal,
    afterCursor: through,
    follow: false,
    toolOutput: true,
    onEvent: (frame) => {
      if (frame.type === 'cma.agent.error') throw new ProtocolError(frame.data.message);
      if (frame.type !== 'cma.agent.event') return;
      index.accept(frame.data);
      through = frame.data.cursor;
    },
  });
  signal.throwIfAborted();
  return index.snapshot(through);
}
