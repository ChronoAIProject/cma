import type { ResolvedConfig } from '../config.js';
import { resolveRequestId } from '../config.js';
import {
  ApiError,
  ProtocolError,
  RequestAborted,
  TransportError,
  ValidationError,
} from '../errors.js';
import { raceWithAbortSignal, type OperationOptions } from '../http.js';
import { read, segment } from '../resources.js';
import { streamLaunchEvents, type LaunchStreamOptions } from '../launch/stream.js';
import { agentSandboxSnapshot, aguiCapabilities } from './companions.js';
import { connection } from './transport.js';
import { commandStreamInput } from '../command-stream-input.js';
import { answerContext } from '../answer-context.js';
import { record } from '../schema.js';
import type {
  AgentConversationEvent,
  AgentReplayOptions,
  AgentRunRequest,
  AgentRunOptions,
  AgentStreamOptions,
} from './types.js';

export type {
  AgentConversationEvent,
  AgentReplayOptions,
  AgentRunRequest,
  AgentRunOptions,
  AgentStreamOptions,
} from './types.js';
export type { AgentSandboxSnapshot, AguiCapabilities } from './companions.js';

/** Agent conversation streaming through the public AG-UI projection. */
export class AgentConversations {
  constructor(private readonly config: ResolvedConfig) {}

  readCapabilities(options: OperationOptions = {}) {
    return read(this.config, '/api/v1/agui/capabilities', aguiCapabilities, options);
  }

  readSandboxSnapshot(threadId: string, options: OperationOptions = {}) {
    return read(
      this.config,
      `/api/v1/agui/agents/${segment(threadId)}/sandbox`,
      (value) => {
        const snapshot = agentSandboxSnapshot(value);
        if (snapshot.threadId !== threadId)
          throw new ProtocolError('AG-UI Sandbox snapshot belongs to another Agent');
        return snapshot;
      },
      options,
    );
  }

  streamLaunchEvents(launchId: string, options: LaunchStreamOptions) {
    return streamLaunchEvents(this.config, launchId, options);
  }

  /** A run is submitted exactly once. A disconnect never resubmits its input. */
  async run(request: AgentRunRequest, options: AgentRunOptions): Promise<void> {
    const captured = commandStreamInput(request, options.idempotencyKey);
    validateThread(captured.body?.threadId);
    if (captured.body.forwardedProps !== undefined) {
      const props = record(captured.body.forwardedProps);
      if (props.answerContext !== undefined) {
        answerContext(props.answerContext);
        if (captured.body.resume?.length)
          throw new ValidationError('Answer context requires user input');
      }
    }
    const stream = connection(
      this.config,
      '/api/v1/agui/run',
      captured.body.threadId,
      {
        ...options,
        timeoutMs: options.timeoutMs ?? 605_000,
      },
      captured.body,
      captured.headers,
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

function validateThread(thread: unknown): asserts thread is string {
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
