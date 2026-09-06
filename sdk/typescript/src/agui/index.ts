import type { ResolvedConfig } from '../config.js';
import { resolveRequestId } from '../config.js';
import {
  ApiError,
  ProtocolError,
  RequestAborted,
  TransportError,
  ValidationError,
} from '../errors.js';
import { raceWithAbortSignal } from '../http.js';
import { connection } from './transport.js';
import type {
  AgentConversationEvent,
  AgentReplayOptions,
  AgentRunRequest,
  AgentStreamOptions,
} from './types.js';

export type {
  AgentConversationEvent,
  AgentReplayOptions,
  AgentRunRequest,
  AgentStreamOptions,
} from './types.js';

/** Agent conversation streaming through the public AG-UI projection. */
export class AgentConversations {
  constructor(private readonly config: ResolvedConfig) {}

  /** A run is submitted exactly once. A disconnect never resubmits its input. */
  async run(request: AgentRunRequest, options: AgentStreamOptions): Promise<void> {
    validateThread(request.threadId);
    const stream = connection(
      this.config,
      '/api/v1/agui/run',
      request.threadId,
      {
        ...options,
        timeoutMs: options.timeoutMs ?? 605_000,
      },
      request,
    );
    let ended = false;
    for await (const events of completeProjections(stream, true)) {
      for (const event of events) {
        await acknowledge(options, event);
        if (event.type === 'RUN_FINISHED' || event.type === 'RUN_ERROR') ended = true;
      }
    }
    if (!ended)
      throw new TransportError('The Agent run stream closed before its outcome was known.');
  }

  /** Only read-only replay reconnects, from the last fully delivered durable projection. */
  async replay(threadId: string, options: AgentReplayOptions): Promise<void> {
    validateThread(threadId);
    let cursor = options.after ?? 0;
    const maxReconnects = options.maxReconnects ?? 4;
    if (
      !Number.isSafeInteger(cursor) ||
      cursor < 0 ||
      !Number.isSafeInteger(maxReconnects) ||
      maxReconnects < 0 ||
      maxReconnects > 20
    ) {
      throw new ValidationError(
        'Replay cursor and reconnect count must be bounded non-negative integers',
      );
    }
    const follow = options.follow ?? true;
    const requestId = await resolveRequestId(this.config, options.requestId);
    let failures = 0;
    while (true) {
      if (options.signal?.aborted === true) throw new RequestAborted();
      const query = new URLSearchParams({
        thread_id: threadId,
        starting_after: String(cursor),
        follow: String(follow),
      });
      try {
        const stream = connection(
          this.config,
          `/api/v1/agui/events?${query.toString()}`,
          threadId,
          { ...options, requestId },
        );
        for await (const events of completeProjections(stream, false)) {
          const nextCursor = events.at(-1)?.cursor;
          if (nextCursor !== undefined && nextCursor <= cursor) {
            throw new ProtocolError('AG-UI replay did not advance its durable cursor');
          }
          for (const event of events) await acknowledge(options, event);
          if (nextCursor !== undefined) {
            cursor = nextCursor;
            failures = 0;
          }
        }
        if (!follow) return;
      } catch (cause) {
        if (
          !follow ||
          !(cause instanceof TransportError || (cause instanceof ApiError && cause.isRetryable))
        )
          throw cause;
      }
      if (failures >= maxReconnects)
        throw new TransportError('The Agent replay closed repeatedly without durable progress.');
      failures += 1;
      await pause(Math.min(500 * 2 ** (failures - 1), 15_000), options.signal);
    }
  }
}

async function acknowledge(
  options: AgentStreamOptions,
  event: AgentConversationEvent,
): Promise<void> {
  try {
    await options.onEvent(event);
  } catch (cause) {
    throw new ProtocolError('The Agent event consumer did not acknowledge a frame', { cause });
  }
}

async function* completeProjections(
  stream: AsyncIterable<AgentConversationEvent>,
  allowBoundary: boolean,
): AsyncGenerator<readonly AgentConversationEvent[]> {
  let pending: AgentConversationEvent[] = [];
  let bytes = 0;
  for await (const event of stream) {
    if (
      event['cma:live'] === true ||
      (allowBoundary && event.type === 'RUN_STARTED' && event.cursor === undefined)
    ) {
      yield [event];
      continue;
    }
    bytes += new TextEncoder().encode(JSON.stringify(event)).byteLength;
    if (pending.length >= 32 || bytes > 4 * 1024 * 1024)
      throw new ProtocolError('AG-UI durable projection exceeds its boundary');
    pending.push(event);
    if (event.cursor !== undefined) {
      yield pending;
      pending = [];
      bytes = 0;
    }
  }
  if (pending.length > 0)
    throw new TransportError('AG-UI disconnected during a durable projection');
}

function validateThread(thread: string): void {
  if (typeof thread !== 'string' || !thread.trim() || thread.length > 256)
    throw new ValidationError('An existing Agent id is required');
}

async function pause(milliseconds: number, signal?: AbortSignal): Promise<void> {
  const local = signal ?? new AbortController().signal;
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    await raceWithAbortSignal(
      local,
      () =>
        new Promise<void>((resolve) => {
          timer = setTimeout(resolve, milliseconds);
        }),
    );
  } catch (cause) {
    throw new RequestAborted(undefined, { cause });
  } finally {
    clearTimeout(timer);
  }
}
