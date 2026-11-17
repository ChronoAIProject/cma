import { createParser } from 'eventsource-parser';
import { CmaError, ProtocolError, TransportError } from '../errors.js';
import { raceWithAbortSignal } from '../http.js';
import {
  boolean,
  enumeration,
  nullable,
  object,
  optional,
  record,
  text,
  type Output,
} from '../schema.js';
import { OutboundMessageSchema, ProblemSchema } from './types.js';
import { mediaFrame, type GatewayMediaControlEvent } from './media-stream.js';

const identity = { event_id: text, conversation_id: text };
const delta = object({ ...identity, text });
const message = object({
  ...identity,
  text: nullable(text),
  message: nullable(OutboundMessageSchema),
  error: nullable(ProblemSchema),
  status: optional(enumeration('completed', 'incomplete')),
  media_complete: optional(boolean),
});

export type GatewayStreamEvent =
  | { readonly event: 'cmaeg.text.delta'; readonly data: Output<typeof delta> }
  | {
      readonly event: 'cmaeg.message' | 'cmaeg.completed' | 'cmaeg.failed';
      readonly data: Output<typeof message>;
    }
  | {
      readonly event: 'cmaeg.failed';
      readonly data: {
        readonly error: { readonly code: 'internal'; readonly detail: 'sync_backpressure' };
      };
    };

/** Superset returned only by explicitly negotiated attachment-response streams. */
export type GatewayMediaStreamEvent = GatewayStreamEvent | GatewayMediaControlEvent;

export async function* gatewayFrames(
  response: Response,
  signal: AbortSignal,
  eventId: string,
  mediaResponses = false,
): AsyncGenerator<GatewayMediaStreamEvent> {
  if (
    response.headers.get('content-type')?.split(';')[0]?.trim().toLowerCase() !==
    'text/event-stream'
  )
    throw new ProtocolError('gateway stream requires text/event-stream');
  if (response.body === null) throw new ProtocolError('gateway stream carried no body');
  const pending: GatewayMediaStreamEvent[] = [];
  let bytes = 0;
  let ended = false;
  let conversation: string | undefined;
  let failure: ProtocolError | undefined;
  const parser = createParser({
    maxBufferSize: 1024 * 1024,
    onEvent(frame) {
      try {
        const size = new TextEncoder().encode(frame.data).byteLength;
        bytes += size;
        if (bytes > 4 * 1024 * 1024 || size > 1024 * 1024 || pending.length >= 1024)
          throw new ProtocolError('gateway stream exceeds its frame or pending boundary');
        if (ended || frame.id !== undefined)
          throw new ProtocolError('gateway stream returned an invalid event envelope');
        const raw: unknown = JSON.parse(frame.data);
        const name = frame.event;
        const media = mediaResponses ? mediaFrame(name ?? '', raw) : undefined;
        if (media !== undefined) {
          if (media.event === 'cmaeg.media.pending') ended = true;
          else {
            if (
              media.data.event_id !== eventId ||
              (conversation !== undefined && media.data.conversation_id !== conversation)
            )
              throw new ProtocolError('gateway stream changed its event or conversation identity');
            conversation = media.data.conversation_id;
          }
          pending.push(media);
          return;
        }
        if (name === 'cmaeg.failed') {
          const error = record(raw).error;
          if (
            error !== null &&
            record(error).code === 'internal' &&
            record(error).detail === 'sync_backpressure'
          ) {
            pending.push({
              event: name,
              data: { error: { code: 'internal', detail: 'sync_backpressure' } },
            });
            ended = true;
            return;
          }
        }
        if (
          name !== 'cmaeg.text.delta' &&
          name !== 'cmaeg.message' &&
          name !== 'cmaeg.completed' &&
          name !== 'cmaeg.failed'
        )
          throw new ProtocolError('gateway stream returned an unknown event');
        const data = name === 'cmaeg.text.delta' ? delta(raw) : message(raw);
        if (
          data.event_id !== eventId ||
          (conversation !== undefined && data.conversation_id !== conversation)
        )
          throw new ProtocolError('gateway stream changed its event or conversation identity');
        conversation = data.conversation_id;
        ended = name === 'cmaeg.completed' || name === 'cmaeg.failed';
        if (name === 'cmaeg.text.delta') pending.push({ event: name, data: delta(raw) });
        else {
          const value = message(raw);
          if (
            (name === 'cmaeg.completed' && (value.message !== null || value.error !== null)) ||
            (name === 'cmaeg.failed' &&
              value.error === null &&
              !(
                mediaResponses &&
                value.status === 'incomplete' &&
                value.media_complete === false
              )) ||
            (value.media_complete !== undefined &&
              (!mediaResponses ||
                value.media_complete !== (name === 'cmaeg.completed') ||
                value.status !== (name === 'cmaeg.completed' ? 'completed' : 'incomplete')))
          )
            throw new ProtocolError('gateway terminal event has an inconsistent outcome');
          pending.push({ event: name, data: value });
        }
      } catch (cause) {
        failure = new ProtocolError('gateway returned an invalid stream frame', { cause });
      }
    },
    onError(cause) {
      failure = new ProtocolError('gateway returned invalid SSE', { cause });
    },
  });
  const decoder = new TextDecoder('utf-8', { fatal: true });
  const reader = response.body.getReader();
  try {
    while (true) {
      const { done, value } = await raceWithAbortSignal(signal, () => reader.read()).catch(
        (cause: unknown) => {
          if (cause instanceof CmaError) throw cause;
          throw new TransportError('gateway stream body was interrupted', { cause });
        },
      );
      if (value !== undefined && value.byteLength > 4 * 1024 * 1024)
        throw new ProtocolError('gateway stream chunk exceeds its boundary');
      try {
        parser.feed(done ? decoder.decode() : decoder.decode(value, { stream: true }));
      } catch (cause) {
        throw new ProtocolError('gateway stream is not valid UTF-8 or SSE', { cause });
      }
      if (failure !== undefined) throw failure;
      while (pending.length > 0) yield pending.shift() as GatewayMediaStreamEvent;
      bytes = 0;
      if (ended) return;
      if (done) throw new TransportError('gateway stream closed before a terminal outcome');
    }
  } finally {
    void reader.cancel().catch(() => undefined);
    reader.releaseLock();
  }
}
