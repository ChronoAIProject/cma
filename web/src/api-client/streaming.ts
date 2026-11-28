import {
  headersFor,
  observeResponse,
  resolveRequestId,
  urlOf,
  type ResolvedConfig,
} from './config.js';
import {
  CmaError,
  ProtocolError,
  RequestAborted,
  TimeoutError,
  TransportError,
  ValidationError,
} from './errors.js';
import { errorFromResponse, raceWithAbortSignal, throwIfSignalAborted } from './http.js';
import type { OperationOptions } from './http.js';

/** The deadline covers admission and response headers, not an active event body. */
export async function* eventStream<Value>(
  config: ResolvedConfig,
  path: string,
  options: OperationOptions,
  decode: (response: Response, signal: AbortSignal) => AsyncIterable<Value>,
  protocol: string,
  body?: unknown,
  extraHeaders?: Readonly<Record<string, string>>,
  onRequestId?: (requestId: string) => void | Promise<void>,
): AsyncGenerator<Value> {
  const timeoutMs = options.timeoutMs ?? config.requestTimeoutMs;
  if (!Number.isSafeInteger(timeoutMs) || timeoutMs <= 0) {
    throw new ValidationError('timeoutMs must be a positive safe integer');
  }
  const callerAborted = () => options.signal?.aborted === true;
  if (callerAborted()) throw new RequestAborted();
  const controller = new AbortController();
  const abort = () => controller.abort(options.signal?.reason);
  options.signal?.addEventListener('abort', abort, { once: true });
  let timedOut = false;
  const timer = setTimeout(() => {
    timedOut = true;
    controller.abort();
  }, timeoutMs);
  let response: Response | undefined;
  try {
    response = await raceWithAbortSignal(controller.signal, async () => {
      const requestId = await resolveRequestId(config, options.requestId);
      await onRequestId?.(requestId);
      const headers = await headersFor(
        config,
        { ...extraHeaders, Accept: 'text/event-stream' },
        { requestId },
      );
      throwIfSignalAborted(controller.signal);
      const received = await config.fetch(urlOf(config, path), {
        method: body === undefined ? 'GET' : 'POST',
        headers,
        body: body === undefined ? undefined : JSON.stringify(body),
        credentials: config.cookies ? 'include' : 'omit',
        redirect: 'error',
        signal: controller.signal,
      });
      const metadata = await observeResponse(config, received);
      throwIfSignalAborted(controller.signal);
      if (!received.ok) {
        throw await errorFromResponse(received, controller.signal, metadata.requestId);
      }
      if (received.status !== 200) throw new ProtocolError(`${protocol} requires HTTP 200`);
      const media = received.headers.get('content-type')?.split(';', 1)[0]?.trim().toLowerCase();
      if (media !== 'text/event-stream' || received.body === null) {
        throw new ProtocolError(`${protocol} requires a text/event-stream body`);
      }
      return received;
    });
    clearTimeout(timer);
    yield* decode(response, controller.signal);
  } catch (cause) {
    if (callerAborted()) throw new RequestAborted(undefined, { cause });
    if (timedOut) throw new TimeoutError(timeoutMs, { cause });
    if (cause instanceof CmaError) throw cause;
    throw new TransportError(`${protocol} event stream could not be read.`, { cause });
  } finally {
    clearTimeout(timer);
    options.signal?.removeEventListener('abort', abort);
    controller.abort();
    if (response?.body?.locked === false) {
      void response.body.cancel().catch(() => undefined);
    }
  }
}
