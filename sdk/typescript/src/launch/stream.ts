import { createParser } from 'eventsource-parser';
import type { ResolvedConfig } from '../config.js';
import { ProtocolError, ValidationError } from '../errors.js';
import { raceWithAbortSignal, requestResponse, type OperationOptions } from '../http.js';
import { segment } from '../resources.js';
import { launchEvent, type LaunchEvent } from './events.js';

export interface LaunchStreamOptions extends OperationOptions {
  readonly after?: number;
  readonly onEvent: (event: LaunchEvent) => void | Promise<void>;
}

/** One bounded companion connection; the caller owns reconnect and its acknowledged cursor. */
export async function streamLaunchEvents(
  config: ResolvedConfig,
  id: string,
  options: LaunchStreamOptions,
): Promise<void> {
  let cursor = options.after ?? 0;
  if (!Number.isSafeInteger(cursor) || cursor < 0)
    throw new ValidationError('The event cursor must be a nonnegative safe integer');
  const signal = options.signal ?? new AbortController().signal;
  const response = await requestResponse(
    config,
    `/api/v1/agui/launches/${segment(id)}/events?starting_after=${String(cursor)}`,
    { ...options, headers: { Accept: 'text/event-stream' }, expectedStatus: 200 },
  );
  if (
    response.headers.get('content-type')?.split(';')[0]?.trim().toLowerCase() !==
      'text/event-stream' ||
    response.body === null
  )
    throw new ProtocolError('The progress companion requires an event stream');
  const pending: LaunchEvent[] = [];
  let failure: unknown;
  const parser = createParser({
    maxBufferSize: 262_144,
    onEvent(message) {
      try {
        const event = launchEvent(JSON.parse(message.data) as unknown);
        if (
          event.launch_id !== id ||
          message.id !== String(event.seq) ||
          event.seq <= cursor ||
          pending.length >= 256
        )
          throw new ProtocolError('Progress event identity or ordering did not match');
        pending.push(event);
      } catch (cause) {
        failure = cause;
      }
    },
    onError(cause) {
      failure = cause;
    },
  });
  const reader = response.body.getReader();
  const decoder = new TextDecoder('utf-8', { fatal: true });
  try {
    while (true) {
      const { done, value } = await raceWithAbortSignal(signal, () => reader.read());
      if (value !== undefined && value.byteLength > 1_048_576)
        throw new ProtocolError('Progress event chunk exceeds its boundary');
      parser.feed(done ? decoder.decode() : decoder.decode(value, { stream: true }));
      if (failure !== undefined)
        throw new ProtocolError('The progress companion returned an invalid event', {
          cause: failure,
        });
      while (pending.length > 0) {
        const event = pending.shift()!;
        if (event.seq <= cursor) throw new ProtocolError('Progress events did not advance');
        await options.onEvent(event);
        cursor = event.seq;
      }
      if (done) return;
    }
  } finally {
    await reader.cancel().catch(() => undefined);
    reader.releaseLock();
  }
}
