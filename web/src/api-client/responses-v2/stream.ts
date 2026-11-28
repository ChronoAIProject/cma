import { createParser, type EventSourceMessage } from 'eventsource-parser';
import { ProtocolError, TransportError } from '../errors.js';
import { raceWithAbortSignal } from '../http.js';
import { v2ResponseEvent, type V2ResponseEvent, type V2ResponseReceipt } from './types.js';
import { V2EventState } from './state.js';

const MAX_FRAME_BYTES = 2 * 1024 * 1024;
const MAX_PENDING_BYTES = 4 * 1024 * 1024;
export interface V2StreamIdentity {
  readonly conversation?: string;
  readonly responseId?: string;
  readonly commandId?: string;
  readonly startingAfter?: number;
  readonly onReceipt?: (receipt: V2ResponseReceipt) => void | Promise<void>;
  readonly eventState?: V2EventState;
  readonly allowEmptyReplay?: boolean;
}

export class V2ObservationError extends TransportError {
  constructor(readonly code: string) {
    super(
      'Response observation ended; retrieve or replay the admitted response with current authorization',
    );
  }
}

export class V2PrematureEof extends TransportError {
  constructor() {
    super('Responses stream closed before its outcome was known');
  }
}

export async function* v2ResponseFrames(
  response: Response,
  signal: AbortSignal,
  expected: V2StreamIdentity,
): AsyncGenerator<V2ResponseEvent> {
  if (response.body === null) throw new ProtocolError('Responses stream carried no body');
  const responseId = response.headers.get('x-cma-response-id');
  const commandId = response.headers.get('x-cma-command-id');
  if (
    !responseId ||
    !commandId ||
    (expected.responseId !== undefined && responseId !== expected.responseId) ||
    (expected.commandId !== undefined && commandId !== expected.commandId)
  )
    throw new ProtocolError('Responses stream has no matching durable receipt');
  try {
    await expected.onReceipt?.({ responseId, commandId });
  } catch (cause) {
    throw new ProtocolError('Responses consumer did not acknowledge its receipt', { cause });
  }
  const state =
    expected.eventState ??
    new V2EventState(responseId, expected.conversation, expected.startingAfter !== undefined);
  const pending: V2ResponseEvent[] = [];
  let pendingBytes = 0;
  let sequence = expected.startingAfter === undefined ? 0 : expected.startingAfter + 1;
  let failure: ProtocolError | undefined;
  let observationError: string | undefined;
  const decode = (message: EventSourceMessage) => {
    const size = new TextEncoder().encode(message.data).byteLength;
    if (size > MAX_FRAME_BYTES || pendingBytes + size > MAX_PENDING_BYTES)
      throw new ProtocolError('Responses frame or pending queue exceeds its boundary');
    const event = v2ResponseEvent(JSON.parse(message.data) as unknown);
    if (
      message.event !== event.type ||
      (event.type === 'error'
        ? message.id !== undefined
        : message.id !== String(event.sequence_number))
    )
      throw new ProtocolError('Responses SSE envelope disagrees with its durable event');
    if (event.sequence_number !== sequence++)
      throw new ProtocolError(
        'Responses event sequence is not contiguous with the requested cursor',
      );
    if (observationError !== undefined || state.ended)
      throw new ProtocolError('Responses frame follows the terminal event');
    if (event.type === 'error') observationError = event.code;
    else state.accept(event);
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
      while (pending.length) yield pending.shift()!;
      pendingBytes = 0;
      if (observationError !== undefined) throw new V2ObservationError(observationError);
      if (done) {
        // A replay immediately after the terminal cursor legitimately has no frames.
        if (
          !state.ended &&
          !(
            (expected.startingAfter !== undefined || expected.allowEmptyReplay) &&
            sequence === (expected.startingAfter === undefined ? 0 : expected.startingAfter + 1)
          )
        )
          throw new V2PrematureEof();
        return;
      }
    }
  } finally {
    void reader.cancel().catch(() => undefined);
    reader.releaseLock();
  }
}
