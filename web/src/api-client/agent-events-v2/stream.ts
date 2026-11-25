import { createParser, type EventSourceMessage } from 'eventsource-parser';
import { ProtocolError, TransportError } from '../errors.js';
import { raceWithAbortSignal } from '../http.js';
import { AgentFrameBound } from './framing.js';
import {
  cursorIncarnation,
  cursorSequence,
  MAX_AGENT_BYTES,
  type V2AgentCursor,
} from './schema.js';
import {
  validateItem,
  v2AgentEvent,
  v2AgentLive,
  v2AgentLiveGap,
  v2AgentStreamError,
  type V2AgentStreamFrame,
} from './types.js';

export async function* agentFrames(
  response: Response,
  signal: AbortSignal,
  agentId: string,
  after?: V2AgentCursor,
  externalApprovals = false,
): AsyncGenerator<V2AgentStreamFrame> {
  if (response.body === null) throw new ProtocolError('Agent stream carried no body');
  let sequence = after === undefined ? 0 : cursorSequence(after);
  let incarnation = after === undefined ? undefined : cursorIncarnation(after);
  let failure: ProtocolError | undefined;
  let pendingBytes = 0;
  const pending: V2AgentStreamFrame[] = [];
  function decode(message: EventSourceMessage) {
    const size = new TextEncoder().encode(message.data).length;
    if (size > MAX_AGENT_BYTES || pendingBytes + size > 2 * MAX_AGENT_BYTES)
      throw new ProtocolError('Agent frame or pending queue exceeds its byte boundary');
    const value: unknown = JSON.parse(message.data);
    let frame: V2AgentStreamFrame;
    switch (message.event) {
      case 'cma.agent.event': {
        const data = v2AgentEvent(value, externalApprovals);
        if (message.id !== data.cursor)
          throw new ProtocolError('Agent durable frame has no matching explicit cursor');
        validateItem(data, incarnation ?? data.incarnation, sequence + 1);
        sequence = data.sequence;
        frame = { type: message.event, data };
        break;
      }
      case 'cma.agent.live': {
        const data = v2AgentLive(value);
        if (data.after_sequence > sequence || data.turn_sequence > data.after_sequence)
          throw new ProtocolError('Agent live fragment precedes its durable anchor');
        frame = { type: message.event, data };
        break;
      }
      case 'cma.agent.live_gap': {
        const data = v2AgentLiveGap(value);
        if (
          (data.reason === 'reconnected' && data.frames !== null) ||
          (data.turn_sequence !== null && data.turn_sequence > sequence)
        )
          throw new ProtocolError('Agent live gap claims an invalid loss or turn');
        frame = { type: message.event, data };
        break;
      }
      case 'cma.agent.error':
        frame = { type: message.event, data: v2AgentStreamError(value) };
        break;
      default:
        throw new ProtocolError('unsupported Agent stream event');
    }
    if (frame.type !== 'cma.agent.event' && message.id !== undefined)
      throw new ProtocolError('ephemeral Agent frame must not acknowledge a cursor');
    if ('agent_id' in frame.data) {
      if (
        frame.data.agent_id !== agentId ||
        (incarnation !== undefined && frame.data.incarnation !== incarnation)
      )
        throw new ProtocolError('Agent stream changed resource identity or incarnation');
      incarnation = frame.data.incarnation;
    }
    pendingBytes += size;
    pending.push(frame);
  }
  const parser = createParser({
    maxBufferSize: MAX_AGENT_BYTES + 1024,
    onEvent(message) {
      try {
        decode(message);
      } catch (cause) {
        failure = new ProtocolError('invalid Agent stream frame', { cause });
      }
    },
    onError(cause) {
      failure = new ProtocolError('invalid Agent SSE framing', { cause });
    },
  });
  const decoder = new TextDecoder('utf-8', { fatal: true });
  const bound = new AgentFrameBound();
  const reader = response.body.getReader();
  try {
    while (true) {
      const { done, value } = await raceWithAbortSignal(signal, () => reader.read());
      if (value !== undefined && value.byteLength > 2 * MAX_AGENT_BYTES)
        throw new ProtocolError('Agent transport chunk exceeds its byte boundary');
      if (value !== undefined) bound.inspect(value);
      // A disconnected browser reader may report EOF, including mid-codepoint.
      // Reject the incomplete frame before flushing the UTF-8 decoder.
      if (done) bound.finish();
      let decoded: string;
      try {
        decoded = done ? decoder.decode() : decoder.decode(value, { stream: true });
      } catch (cause) {
        // Streaming decoding already rejects malformed bytes. A failed final
        // UTF-8 flush means EOF interrupted a codepoint, even in an SSE comment.
        if (done)
          throw new TransportError('Agent stream closed inside a UTF-8 codepoint', { cause });
        throw new ProtocolError('invalid Agent UTF-8', { cause });
      }
      try {
        parser.feed(decoded);
      } catch (cause) {
        throw new ProtocolError('invalid Agent SSE', { cause });
      }
      if (failure !== undefined) throw failure;
      while (pending.length) yield pending.shift()!;
      pendingBytes = 0;
      if (done) return;
    }
  } finally {
    void reader.cancel().catch(() => undefined);
    reader.releaseLock();
  }
}
