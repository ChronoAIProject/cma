import type { ResolvedConfig } from '../config.js';
import { ApiError, ProtocolError, ValidationError } from '../errors.js';
import type { OperationOptions } from '../http.js';
import { query, read, segment } from '../resources.js';
import { array, integer, object, optional, positiveInteger, text, type Output } from '../schema.js';
import {
  agentTranscript,
  transcriptTurn,
  type AgentTranscript,
  type TranscriptTurn,
  type TranscriptMessage,
} from './types.js';

const cursor = object({
  turn: positiveInteger,
  after: integer,
  through: positiveInteger,
  limit: positiveInteger,
});
export type TranscriptCursor = Output<typeof cursor>;
export interface TranscriptPageOptions extends OperationOptions {
  readonly cursor?: TranscriptCursor;
  readonly limit?: number;
}
const pageShape = object({
  agent_id: text,
  through_seq: integer,
  turns: array(transcriptTurn),
  next: optional(cursor),
});
export type TranscriptPage = Output<typeof pageShape>;

export async function readTranscriptPage(
  config: ResolvedConfig,
  id: string,
  options: TranscriptPageOptions = {},
): Promise<TranscriptPage> {
  const limit = options.cursor?.limit ?? options.limit ?? 32;
  if (!Number.isSafeInteger(limit) || limit < 1 || limit > 32)
    throw new ValidationError('transcript page limit must be 1..32');
  if (options.cursor !== undefined) {
    const c = options.cursor;
    if (
      ![c.turn, c.after, c.through].every(Number.isSafeInteger) ||
      c.turn < 1 ||
      c.after < c.turn - 1 ||
      c.after >= c.through
    )
      throw new ValidationError('invalid transcript continuation');
  }
  return read(
    config,
    query(`/api/v1/agents/${segment(id)}/transcript`, { ...options.cursor, limit }),
    (raw) => {
      const page = pageShape(raw);
      if (page.agent_id !== id || (options.cursor && page.through_seq !== options.cursor.through))
        throw new ProtocolError('transcript page source changed');
      const next = page.next;
      if (
        next &&
        (next.limit !== limit ||
          next.through !== page.through_seq ||
          next.after >= next.through ||
          next.after < next.turn - 1 ||
          next.after <= (options.cursor?.after ?? 0))
      )
        throw new ProtocolError('transcript cursor did not advance within its source');
      return page;
    },
    options,
  );
}

/** Compatibility hydration; explicit page callers may render each bounded response. */
export async function readTranscript(
  config: ResolvedConfig,
  id: string,
  options: OperationOptions,
): Promise<AgentTranscript> {
  try {
    return await read(
      config,
      `/api/v1/agents/${segment(id)}/transcript`,
      (raw) => {
        const value = agentTranscript(raw);
        if (value.agent_id !== id)
          throw new ProtocolError('projection Agent identity did not match the request');
        return value;
      },
      options,
    );
  } catch (error) {
    if (!(error instanceof ApiError) || error.code !== 'transcript_pagination_required')
      throw error;
  }
  let next: TranscriptCursor | undefined;
  let through: number | undefined;
  const turns: (Omit<TranscriptTurn, 'messages' | 'agent_messages'> & {
    messages: TranscriptMessage[];
    agent_messages: string[];
  })[] = [];
  do {
    const page = await readTranscriptPage(config, id, { ...options, cursor: next });
    if (through !== undefined && through !== page.through_seq)
      throw new ProtocolError('transcript snapshot changed');
    through = page.through_seq;
    for (const turn of page.turns) {
      const previous = turns.at(-1);
      if (previous && turn.turn_sequence < previous.turn_sequence)
        throw new ProtocolError('transcript turns moved backwards');
      if (previous?.turn_sequence === turn.turn_sequence) {
        if (
          previous.started_at !== turn.started_at ||
          previous.owner_message !== turn.owner_message ||
          JSON.stringify(previous.outcome) !== JSON.stringify(turn.outcome)
        )
          throw new ProtocolError('transcript turn metadata changed');
        const last = previous.messages.at(-1)?.sequence ?? 0;
        validateMessages(turn, last, through);
        for (const message of turn.messages) previous.messages.push(message);
        for (const message of turn.agent_messages) previous.agent_messages.push(message);
      } else {
        validateMessages(turn, 0, through);
        turns.push({
          ...turn,
          messages: [...turn.messages],
          agent_messages: [...turn.agent_messages],
        });
      }
    }
    next = page.next;
  } while (next !== undefined);
  return { agent_id: id, through_seq: through, turns };
}

function validateMessages(turn: TranscriptTurn, after: number, through: number): void {
  for (const message of turn.messages) {
    if (message.sequence <= after || message.sequence > through)
      throw new ProtocolError('transcript message order is invalid');
    after = message.sequence;
  }
  if (
    JSON.stringify(turn.agent_messages) !==
    JSON.stringify(turn.messages.filter((m) => m.role === 'assistant').map((m) => m.content))
  )
    throw new ProtocolError('transcript assistant summaries disagree with source messages');
}
