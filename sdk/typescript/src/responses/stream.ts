import { createParser, type EventSourceMessage } from 'eventsource-parser';
import { ProtocolError, TransportError } from '../errors.js';
import { raceWithAbortSignal } from '../http.js';
import { responseEvent, type ResponseEvent } from './events.js';

const MAX_FRAME_BYTES = 2 * 1024 * 1024;
const MAX_PENDING_BYTES = 4 * 1024 * 1024;

export async function* responseFrames(
  response: Response,
  signal: AbortSignal,
  conversation: string,
): AsyncGenerator<ResponseEvent> {
  if (response.body === null) throw new ProtocolError('Responses stream carried no body');
  const pending: ResponseEvent[] = [];
  let pendingBytes = 0;
  let sequence = 0;
  let responseId: string | undefined;
  let ended = false;
  let failure: ProtocolError | undefined;
  const decode = (message: EventSourceMessage) => {
    const size = new TextEncoder().encode(message.data).byteLength;
    if (size > MAX_FRAME_BYTES || pendingBytes + size > MAX_PENDING_BYTES)
      throw new ProtocolError('Responses frame or pending queue exceeds its boundary');
    const event = responseEvent(JSON.parse(message.data) as unknown);
    if (message.id !== undefined || (message.event !== undefined && message.event !== event.type))
      throw new ProtocolError('Responses event envelope does not match its payload');
    if (ended || event.sequence_number !== sequence++)
      throw new ProtocolError(
        'Responses connection sequence must begin at zero and advance once per frame',
      );
    if ('response' in event) {
      const prefix = `resp_${conversation}_`;
      const suffix = event.response.id.slice(prefix.length);
      if (
        !event.response.id.startsWith(prefix) ||
        !/^(0|[1-9]\d*)$/.test(suffix) ||
        (responseId !== undefined && event.response.id !== responseId)
      )
        throw new ProtocolError('Responses frame belongs to another Agent or response');
      responseId = event.response.id;
      const status =
        event.type === 'response.created' ? 'in_progress' : event.type.slice('response.'.length);
      if (event.response.status !== status)
        throw new ProtocolError('Responses lifecycle event disagrees with its resource status');
      ended = ['completed', 'failed', 'incomplete'].includes(status);
    }
    if (responseId === undefined || ('response_id' in event && event.response_id !== responseId))
      throw new ProtocolError('Responses stream has no matching response identity');
    pendingBytes += size;
    pending.push(event);
  };
  const parser = createParser({
    maxBufferSize: MAX_FRAME_BYTES,
    onEvent(message) {
      try {
        decode(message);
      } catch (cause) {
        failure = new ProtocolError('Responses returned an invalid frame', { cause });
      }
    },
    onError(cause) {
      failure = new ProtocolError('Responses returned invalid SSE', { cause });
    },
  });
  const decoder = new TextDecoder('utf-8', { fatal: true });
  const reader = response.body.getReader();
  try {
    while (true) {
      const { done, value } = await raceWithAbortSignal(signal, () => reader.read());
      if (value !== undefined && value.byteLength > MAX_PENDING_BYTES)
        throw new ProtocolError('Responses transport chunk exceeds its boundary');
      try {
        parser.feed(done ? decoder.decode() : decoder.decode(value, { stream: true }));
      } catch (cause) {
        throw new ProtocolError('Responses stream is not valid UTF-8 or SSE', { cause });
      }
      if (failure !== undefined) throw failure;
      while (pending.length > 0) yield pending.shift() as ResponseEvent;
      pendingBytes = 0;
      if (done) {
        if (!ended)
          throw new TransportError('Responses stream closed before its outcome was known');
        return;
      }
    }
  } finally {
    void reader.cancel().catch(() => undefined);
    reader.releaseLock();
  }
}
