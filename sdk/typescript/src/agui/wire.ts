import { createParser, type EventSourceMessage } from 'eventsource-parser';
import {
  headersFor,
  observeResponse,
  resolveConfig,
  resolveRequestId,
  urlOf,
  type ClientConfig,
  type ResolvedConfig,
} from '../config.js';
import {
  CmaError,
  ProtocolError,
  RequestAborted,
  TimeoutError,
  TransportError,
  ValidationError,
} from '../errors.js';
import {
  errorFromResponse,
  raceWithAbortSignal,
  throwIfSignalAborted,
  withOperationDeadline,
  type OperationOptions,
} from '../http.js';
import type { AguiTransport } from './connection.js';

const MAX_FRAME_BYTES = 2 * 1024 * 1024;

function streamPath(session: string, cursor: number, follow: boolean): string {
  const query = new URLSearchParams({ thread_id: session });
  if (cursor > 0) {
    query.set('starting_after', String(cursor));
  }
  if (!follow) {
    query.set('follow', 'false');
  }
  return `/api/v1/agui/events?${query.toString()}`;
}

/** Build AG-UI's fetch/ReadableStream transport for browsers and Node. */
export function createAguiTransport(
  configOrOptions: ResolvedConfig | ClientConfig = {},
): AguiTransport {
  const config = isResolved(configOrOptions) ? configOrOptions : resolveConfig(configOrOptions);
  const requestIds = new WeakMap<OperationOptions, Promise<string>>();
  const requestIdFor = (options: OperationOptions): Promise<string> => {
    const existing = requestIds.get(options);
    if (existing !== undefined) {
      return existing;
    }
    const pending = resolveRequestId(config, options.requestId);
    requestIds.set(options, pending);
    return pending;
  };
  return {
    open(session, cursor, onFrame, onDrop, options = {}) {
      const controller = new AbortController();
      const onAbort = () => controller.abort(options.signal?.reason);
      if (options.signal?.aborted === true) {
        controller.abort(options.signal.reason);
      } else {
        options.signal?.addEventListener('abort', onAbort, { once: true });
      }
      let closed = false;

      void (async () => {
        try {
          const response = await openStream(config, session, cursor, options, controller, () =>
            requestIdFor(options),
          );
          for await (const frame of framesFromBody(response, controller.signal)) {
            if (closed) {
              return;
            }
            onFrame(frame);
          }
          if (!closed) {
            onDrop(new TransportError('the AG-UI event stream closed'));
          }
        } catch (cause) {
          if (closed) {
            return;
          }
          if (options.signal?.aborted === true) {
            onDrop(new RequestAborted(undefined, { cause }));
          } else {
            onDrop(
              cause instanceof CmaError
                ? cause
                : new TransportError(`the AG-UI event stream failed: ${messageOf(cause)}`, {
                    cause,
                  }),
            );
          }
        } finally {
          options.signal?.removeEventListener('abort', onAbort);
        }
      })();

      return () => {
        closed = true;
        options.signal?.removeEventListener('abort', onAbort);
        controller.abort();
      };
    },

    async history(session, cursor, options = {}) {
      return withOperationDeadline(config, options, async (signal) => {
        const requestId = await requestIdFor(options);
        throwIfSignalAborted(signal);
        const headers = await headersFor(config, { Accept: 'text/event-stream' }, { requestId });
        throwIfSignalAborted(signal);
        const response = await config.fetch(urlOf(config, streamPath(session, cursor, false)), {
          method: 'GET',
          headers,
          credentials: config.cookies ? 'include' : 'omit',
          redirect: 'error',
          signal,
        });
        throwIfSignalAborted(signal);
        const metadata = await observeResponse(config, response);
        throwIfSignalAborted(signal);
        await validateResponse(response, signal, metadata.requestId);
        throwIfSignalAborted(signal);
        const frames: unknown[] = [];
        for await (const frame of framesFromBody(response, signal)) {
          frames.push(frame);
        }
        return frames;
      });
    },
  };
}

/** Same-origin cookie transport retained for the reference dashboard. */
export const wireTransport = createAguiTransport();

async function openStream(
  config: ResolvedConfig,
  session: string,
  cursor: number,
  options: OperationOptions,
  controller: AbortController,
  requestId: () => Promise<string>,
): Promise<Response> {
  const timeoutMs = options.timeoutMs ?? config.requestTimeoutMs;
  if (!Number.isSafeInteger(timeoutMs) || timeoutMs <= 0) {
    throw new ValidationError('timeoutMs must be a positive safe integer');
  }
  let timedOut = false;
  const timer = setTimeout(() => {
    timedOut = true;
    controller.abort();
  }, timeoutMs);
  try {
    const response = await raceWithAbortSignal(controller.signal, async () => {
      const operationRequestId = await requestId();
      throwIfSignalAborted(controller.signal);
      const headers = await headersFor(
        config,
        {
          Accept: 'text/event-stream',
          ...(cursor === 0 ? {} : { 'Last-Event-ID': String(cursor) }),
        },
        { requestId: operationRequestId },
      );
      throwIfSignalAborted(controller.signal);
      const received = await config.fetch(urlOf(config, streamPath(session, cursor, true)), {
        method: 'GET',
        headers,
        credentials: config.cookies ? 'include' : 'omit',
        redirect: 'error',
        signal: controller.signal,
      });
      throwIfSignalAborted(controller.signal);
      const metadata = await observeResponse(config, received);
      throwIfSignalAborted(controller.signal);
      await validateResponse(received, controller.signal, metadata.requestId);
      throwIfSignalAborted(controller.signal);
      return received;
    });
    clearTimeout(timer);
    return response;
  } catch (cause) {
    clearTimeout(timer);
    controller.abort();
    if (options.signal?.aborted === true) {
      throw new RequestAborted(undefined, { cause });
    }
    if (timedOut) {
      throw new TimeoutError(timeoutMs, { cause });
    }
    if (cause instanceof CmaError) {
      throw cause;
    }
    throw new TransportError(`cannot open the AG-UI event stream: ${messageOf(cause)}`, { cause });
  }
}

async function validateResponse(
  response: Response,
  signal: AbortSignal,
  requestId?: string,
): Promise<void> {
  if (!response.ok) {
    throw await errorFromResponse(response, signal, requestId);
  }
  if (response.status !== 200) {
    throw new ProtocolError(
      `AG-UI event stream expected HTTP 200 but received ${String(response.status)}`,
    );
  }
  const contentType = response.headers.get('content-type') ?? '';
  if (contentType.split(';', 1)[0]?.trim().toLowerCase() !== 'text/event-stream') {
    throw new ProtocolError(`AG-UI event stream returned content type '${contentType}'`);
  }
  if (response.body === null) {
    throw new ProtocolError('AG-UI event stream response carried no body');
  }
}

async function* framesFromBody(response: Response, signal: AbortSignal): AsyncGenerator<unknown> {
  const body = response.body;
  if (body === null) {
    throw new ProtocolError('AG-UI event stream response carried no body');
  }
  const pending: unknown[] = [];
  let failure: ProtocolError | undefined;
  const parser = parserFor(
    (frame) => pending.push(frame),
    (error) => {
      failure = error;
    },
  );
  const decoder = new TextDecoder('utf-8', { fatal: true });
  const reader = body.getReader();
  try {
    while (true) {
      const { done, value } = await raceWithAbortSignal(signal, () => reader.read());
      if (done) {
        break;
      }
      try {
        parser.feed(decoder.decode(value, { stream: true }));
      } catch (cause) {
        throw new ProtocolError('AG-UI event stream is not valid UTF-8 or SSE', { cause });
      }
      if (failure !== undefined) {
        throw failure;
      }
      while (pending.length > 0) {
        yield pending.shift();
      }
    }
    try {
      parser.feed(decoder.decode());
      parser.reset({ consume: true });
    } catch (cause) {
      throw new ProtocolError('AG-UI event stream ended with an invalid frame', { cause });
    }
    if (failure !== undefined) {
      throw failure;
    }
    while (pending.length > 0) {
      yield pending.shift();
    }
  } finally {
    if (signal.aborted) {
      await reader.cancel(signal.reason).catch(() => undefined);
    }
    reader.releaseLock();
  }
}

/** Parse a complete SSE body with the same strict boundary as the live stream. */
export function parseEventStream(body: string): unknown[] {
  const frames: unknown[] = [];
  let failure: ProtocolError | undefined;
  const parser = parserFor(
    (frame) => frames.push(frame),
    (error) => {
      failure = error;
    },
  );
  try {
    parser.feed(body);
    parser.reset({ consume: true });
  } catch (cause) {
    throw new ProtocolError('AG-UI event stream was not valid SSE', { cause });
  }
  if (failure !== undefined) {
    throw failure;
  }
  return frames;
}

function parserFor(onFrame: (frame: unknown) => void, onFailure: (error: ProtocolError) => void) {
  return createParser({
    maxBufferSize: MAX_FRAME_BYTES,
    onEvent: (message) => {
      try {
        onFrame(frameFromMessage(message));
      } catch (cause) {
        onFailure(
          cause instanceof ProtocolError
            ? cause
            : new ProtocolError('AG-UI frame could not be decoded', { cause }),
        );
      }
    },
    onError: (error) =>
      onFailure(
        new ProtocolError(`AG-UI stream is not valid SSE: ${error.message}`, { cause: error }),
      ),
  });
}

function frameFromMessage(message: EventSourceMessage): unknown {
  if (new TextEncoder().encode(message.data).byteLength > MAX_FRAME_BYTES) {
    throw new ProtocolError(`AG-UI frame exceeds the ${String(MAX_FRAME_BYTES)}-byte boundary`);
  }
  let raw: unknown;
  try {
    raw = JSON.parse(message.data) as unknown;
  } catch (cause) {
    throw new ProtocolError('AG-UI frame was not valid JSON', { cause });
  }
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) {
    throw new ProtocolError('AG-UI frame was not an object');
  }
  const frame = raw as Record<string, unknown>;
  const type = typeof frame.type === 'string' && frame.type !== '' ? frame.type : undefined;
  if (type === undefined) {
    throw new ProtocolError("AG-UI frame required a non-empty string 'type'");
  }
  if (message.event !== undefined && message.event !== 'message' && message.event !== type) {
    throw new ProtocolError(
      `AG-UI SSE event name '${message.event}' did not match payload type '${type}'`,
    );
  }
  if (frame['cma:live'] === true) {
    if (message.id !== undefined && message.id !== '') {
      throw new ProtocolError('AG-UI liveness frame unexpectedly carried a sequence id');
    }
    return { ...frame, seq: 0 };
  }
  return { ...frame, seq: sequenceOf(message.id) };
}

function sequenceOf(raw: string | undefined): number {
  if (raw === undefined || !/^[1-9]\d*$/.test(raw)) {
    throw new ProtocolError(`AG-UI durable frame id '${raw ?? ''}' was not a positive sequence`);
  }
  const value = Number(raw);
  if (!Number.isSafeInteger(value)) {
    throw new ProtocolError(`AG-UI durable frame id '${raw}' exceeded the safe integer range`);
  }
  return value;
}

function isResolved(value: ResolvedConfig | ClientConfig): value is ResolvedConfig {
  return (
    typeof (value as Partial<ResolvedConfig>).baseUrl === 'string' &&
    typeof (value as Partial<ResolvedConfig>).fetch === 'function' &&
    typeof (value as Partial<ResolvedConfig>).requestIdProvider === 'function' &&
    typeof (value as Partial<ResolvedConfig>).requestTimeoutMs === 'number' &&
    typeof (value as Partial<ResolvedConfig>).cookies === 'boolean'
  );
}

function messageOf(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}
