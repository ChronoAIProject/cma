import { createParser, type EventSourceMessage } from 'eventsource-parser';
import { ProtocolError } from '../errors.js';
import { raceWithAbortSignal } from '../http.js';
import { record, text } from '../schema.js';
import type { AgentConversationEvent } from './types.js';

const MAX_FRAME_BYTES = 2 * 1024 * 1024;
const MAX_PENDING_BYTES = 4 * 1024 * 1024;

/** Frame decoding preserves the distinction between an SSE id and live content. */
export async function* frames(
  response: Response,
  signal: AbortSignal,
  threadId: string,
): AsyncGenerator<AgentConversationEvent> {
  if (response.body === null) throw new ProtocolError('AG-UI response carried no body');
  const pending: AgentConversationEvent[] = [];
  let pendingBytes = 0;
  let failure: ProtocolError | undefined;
  const parser = createParser({
    maxBufferSize: MAX_FRAME_BYTES,
    onEvent(message) {
      try {
        pendingBytes += new TextEncoder().encode(message.data).byteLength;
        if (pendingBytes > MAX_PENDING_BYTES) throw new ProtocolError('AG-UI frame queue is full');
        pending.push(decode(message, threadId));
      } catch (cause) {
        failure = new ProtocolError('AG-UI returned an invalid frame', { cause });
      }
    },
    onError(cause) {
      failure = new ProtocolError('AG-UI returned invalid SSE', { cause });
    },
  });
  const decoder = new TextDecoder('utf-8', { fatal: true });
  const reader = response.body.getReader();
  try {
    while (true) {
      const { done, value } = await raceWithAbortSignal(signal, () => reader.read());
      if (value !== undefined && value.byteLength > MAX_PENDING_BYTES) {
        throw new ProtocolError('AG-UI transport chunk exceeds the decoding boundary');
      }
      try {
        parser.feed(done ? decoder.decode() : decoder.decode(value, { stream: true }));
      } catch (cause) {
        throw new ProtocolError('AG-UI stream is not valid UTF-8 or SSE', { cause });
      }
      if (failure !== undefined) throw failure;
      while (pending.length > 0) yield pending.shift() as AgentConversationEvent;
      pendingBytes = 0;
      if (done) return;
    }
  } finally {
    void reader.cancel().catch(() => undefined);
    reader.releaseLock();
  }
}

function decode(message: EventSourceMessage, threadId: string): AgentConversationEvent {
  if (new TextEncoder().encode(message.data).byteLength > MAX_FRAME_BYTES) {
    throw new ProtocolError('AG-UI frame exceeds the decoding boundary');
  }
  const body = record(JSON.parse(message.data) as unknown);
  const type = text(body.type);
  const thread = text(body.threadId);
  const runId = text(body.runId);
  if (!type || !runId || thread !== threadId) {
    throw new ProtocolError('AG-UI frame identity does not match this Agent');
  }
  if (message.event !== undefined && message.event !== 'message' && message.event !== type) {
    throw new ProtocolError('AG-UI SSE event name does not match its payload');
  }
  if (body['cma:live'] !== undefined && typeof body['cma:live'] !== 'boolean') {
    throw new ProtocolError('AG-UI liveness must be boolean');
  }
  if (body['cma:live'] === true && message.id !== undefined) {
    throw new ProtocolError('AG-UI live content cannot carry a durable cursor');
  }
  let cursor: number | undefined;
  if (message.id !== undefined) {
    cursor = Number(message.id);
    if (!/^[1-9]\d*$/.test(message.id) || !Number.isSafeInteger(cursor)) {
      throw new ProtocolError('AG-UI cursor must be a canonical positive safe integer');
    }
  }
  if (body.timestamp !== undefined) text(body.timestamp);
  return { ...body, type, threadId: thread, runId, cursor };
}
