import { createParser, type EventSourceMessage } from 'eventsource-parser';
import { headersFor, urlOf, type ResolvedConfig } from './config.js';
import {
  ApiError,
  CmaError,
  ProtocolError,
  RequestAborted,
  TimeoutError,
  TransportError,
  ValidationError,
} from './errors.js';
import { errorFromResponse, type OperationOptions } from './http.js';

const MAX_EVENT_BYTES = 2 * 1024 * 1024;
const DEFAULT_INITIAL_BACKOFF_MS = 150;
const DEFAULT_MAX_BACKOFF_MS = 3_000;
const DEFAULT_JITTER_RATIO = 0.2;
const DEFAULT_MAX_EMPTY_CONNECTIONS = 4;

export const KNOWN_SESSION_EVENT_TYPES = [
  'session_created',
  'progress',
  'status_changed',
  'turn_started',
  'turn_completed',
  'terminal_attached',
  'terminal_detached',
  'terminal_resized',
  'conversation_opened',
  'message_committed',
  'owner_message',
  'tool_call_started',
  'tool_call_updated',
  'usage_reported',
  'context_compacted',
  'mode_changed',
  'plan_set',
  'config_options_set',
  'available_commands_set',
  'conversation_gap',
  'sign_in_required',
  'approval_requested',
  'approval_resolved',
  'plane_changed',
  'memory_pressure_opened',
  'memory_pressure_closed',
  'runtime_lost',
] as const;

export type KnownSessionEventType = (typeof KNOWN_SESSION_EVENT_TYPES)[number];

interface EventKindBase {
  readonly type: KnownSessionEventType;
  readonly [field: string]: unknown;
}

export interface UnknownSessionEventKind {
  readonly type: 'unknown';
  readonly eventType: string;
  readonly raw: Readonly<Record<string, unknown>>;
}

export type SessionEventKind = EventKindBase | UnknownSessionEventKind;

/** One durable core event, validated at the SSE boundary. */
export interface SessionEvent {
  readonly seq: number;
  readonly sessionId: string;
  readonly event: SessionEventKind;
}

export interface StreamEventsOptions extends OperationOptions {
  /** Resume strictly after this per-session sequence. */
  readonly startingAfter?: number;
  readonly initialBackoffMs?: number;
  readonly maxBackoffMs?: number;
  /** Fractional random spread around each exponential delay, from zero to one. */
  readonly jitterRatio?: number;
  /** Stop after this many consecutive connections deliver no event. */
  readonly maxEmptyConnections?: number;
  /** Test hooks; production callers normally leave both absent. */
  readonly random?: () => number;
  readonly sleep?: (milliseconds: number) => Promise<void>;
}

/**
 * Open a typed, resumable REST event stream.
 *
 * Every reconnect sends the last validated sequence in both the REST query and
 * `Last-Event-ID`, re-resolves credentials, and deduplicates the one overlap a
 * proxy or rolling server may legitimately produce.
 */
export function streamEvents(
  config: ResolvedConfig,
  sessionId: string,
  options: StreamEventsOptions = {},
): AsyncIterable<SessionEvent> {
  const policy = streamPolicy(options);
  return reconnectingEvents(config, sessionId, options, policy);
}

interface StreamPolicy {
  readonly initialBackoffMs: number;
  readonly maxBackoffMs: number;
  readonly jitterRatio: number;
  readonly maxEmptyConnections: number;
  readonly random: () => number;
  readonly sleep: (milliseconds: number) => Promise<void>;
}

async function* reconnectingEvents(
  config: ResolvedConfig,
  sessionId: string,
  options: StreamEventsOptions,
  policy: StreamPolicy,
): AsyncGenerator<SessionEvent> {
  let cursor = options.startingAfter ?? 0;
  let emptyConnections = 0;
  let failedAttempts = 0;
  let serverRetryMs: number | undefined;

  while (true) {
    throwIfAborted(options.signal);
    let active: ActiveResponse;
    try {
      active = await connect(config, sessionId, cursor, options);
    } catch (cause) {
      if (!retryable(cause)) {
        throw cause;
      }
      failedAttempts += 1;
      if (failedAttempts >= policy.maxEmptyConnections) {
        throw cause;
      }
      await waitBeforeReconnect(policy, failedAttempts, undefined, options.signal);
      continue;
    }

    let received = false;
    let disconnected = false;
    try {
      for await (const frame of parseStream(active.response, sessionId, (retry) => {
        serverRetryMs = retry;
      })) {
        if (frame.seq < cursor) {
          throw new ProtocolError(
            `event sequence ${String(frame.seq)} moved behind resume cursor ${String(cursor)}`,
          );
        }
        if (frame.seq === cursor) {
          continue;
        }
        cursor = frame.seq;
        received = true;
        emptyConnections = 0;
        failedAttempts = 0;
        yield frame;
      }
      disconnected = true;
    } catch (cause) {
      if (options.signal?.aborted === true) {
        throw new RequestAborted(undefined, { cause });
      }
      if (cause instanceof ProtocolError || cause instanceof ApiError) {
        throw cause;
      }
      disconnected = true;
    } finally {
      active.close();
    }

    if (!disconnected) {
      return;
    }
    if (!received) {
      emptyConnections += 1;
      failedAttempts += 1;
      if (emptyConnections >= policy.maxEmptyConnections) {
        throw new TransportError('event stream closed repeatedly without delivering an event');
      }
    }
    await waitBeforeReconnect(policy, Math.max(failedAttempts, 1), serverRetryMs, options.signal);
  }
}

interface ActiveResponse {
  readonly response: Response;
  close(): void;
}

async function connect(
  config: ResolvedConfig,
  sessionId: string,
  cursor: number,
  options: OperationOptions,
): Promise<ActiveResponse> {
  const timeoutMs = options.timeoutMs ?? config.requestTimeoutMs;
  if (!positiveInteger(timeoutMs)) {
    throw new ValidationError('timeoutMs must be a positive safe integer');
  }
  const controller = new AbortController();
  let timedOut = false;
  const onAbort = () => controller.abort(options.signal?.reason);
  options.signal?.addEventListener('abort', onAbort, { once: true });
  const timer = setTimeout(() => {
    timedOut = true;
    controller.abort();
  }, timeoutMs);
  const query = cursor === 0 ? '' : `?after=${String(cursor)}`;

  try {
    const response = await config.fetch(
      urlOf(config, `/api/v1/sessions/${encodeURIComponent(sessionId)}/events${query}`),
      {
        method: 'GET',
        headers: await headersFor(config, {
          Accept: 'text/event-stream',
          ...(cursor === 0 ? {} : { 'Last-Event-ID': String(cursor) }),
        }),
        credentials: config.cookies ? 'include' : 'omit',
        redirect: 'error',
        signal: controller.signal,
      },
    );
    if (!response.ok) {
      throw await errorFromResponse(response);
    }
    if (response.status !== 200) {
      throw new ProtocolError(
        `event stream expected HTTP 200 but received ${String(response.status)}`,
      );
    }
    const contentType = response.headers.get('content-type') ?? '';
    if (contentType.split(';', 1)[0]?.trim().toLowerCase() !== 'text/event-stream') {
      throw new ProtocolError(`event stream returned content type '${contentType}'`);
    }
    if (response.body === null) {
      throw new ProtocolError('event stream response carried no body');
    }
    clearTimeout(timer);
    return {
      response,
      close: () => {
        options.signal?.removeEventListener('abort', onAbort);
        controller.abort();
      },
    };
  } catch (cause) {
    clearTimeout(timer);
    options.signal?.removeEventListener('abort', onAbort);
    controller.abort();
    if (cause instanceof CmaError) {
      throw cause;
    }
    if (options.signal?.aborted === true) {
      throw new RequestAborted(undefined, { cause });
    }
    if (timedOut) {
      throw new TimeoutError(timeoutMs, { cause });
    }
    throw new TransportError(`cannot open the event stream: ${messageOf(cause)}`, { cause });
  }
}

async function* parseStream(
  response: Response,
  sessionId: string,
  onRetry: (milliseconds: number) => void,
): AsyncGenerator<SessionEvent> {
  const body = response.body;
  if (body === null) {
    throw new ProtocolError('event stream response carried no body');
  }
  const pending: SessionEvent[] = [];
  let failure: ProtocolError | undefined;
  const parser = createParser({
    maxBufferSize: MAX_EVENT_BYTES,
    onEvent: (message) => {
      try {
        pending.push(eventFromMessage(message, sessionId));
      } catch (cause) {
        failure =
          cause instanceof ProtocolError
            ? cause
            : new ProtocolError('event stream frame could not be decoded', { cause });
      }
    },
    onRetry,
    onError: (error) => {
      failure = new ProtocolError(`event stream is not valid SSE: ${error.message}`, {
        cause: error,
      });
    },
  });
  const decoder = new TextDecoder('utf-8', { fatal: true });
  const reader = body.getReader();
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) {
        break;
      }
      try {
        parser.feed(decoder.decode(value, { stream: true }));
      } catch (cause) {
        throw new ProtocolError('event stream is not valid UTF-8 or SSE', { cause });
      }
      if (failure !== undefined) {
        throw failure;
      }
      while (pending.length > 0) {
        yield pending.shift() as SessionEvent;
      }
    }
    try {
      parser.feed(decoder.decode());
      parser.reset({ consume: true });
    } catch (cause) {
      throw new ProtocolError('event stream ended with an invalid frame', { cause });
    }
    if (failure !== undefined) {
      throw failure;
    }
    while (pending.length > 0) {
      yield pending.shift() as SessionEvent;
    }
  } finally {
    reader.releaseLock();
  }
}

/** Validate one SSE message and retain unknown event kinds losslessly. */
export function eventFromMessage(message: EventSourceMessage, sessionId: string): SessionEvent {
  if (message.event !== undefined && message.event !== 'message') {
    throw new ProtocolError(`event stream used unsupported named event '${message.event}'`);
  }
  const sequence = positiveSequence(message.id);
  if (new TextEncoder().encode(message.data).byteLength > MAX_EVENT_BYTES) {
    throw new ProtocolError(`event frame exceeds the ${String(MAX_EVENT_BYTES)}-byte boundary`);
  }
  let raw: unknown;
  try {
    raw = JSON.parse(message.data) as unknown;
  } catch (cause) {
    throw new ProtocolError('event frame was not valid JSON', { cause });
  }
  const envelope = objectOf(raw, 'event frame');
  if (envelope.seq !== sequence) {
    throw new ProtocolError(
      `event frame id ${String(sequence)} did not match payload sequence ${String(envelope.seq)}`,
    );
  }
  if (envelope.session_id !== sessionId) {
    throw new ProtocolError(
      `event for ${String(envelope.session_id)} arrived on stream for ${sessionId}`,
    );
  }
  const event = objectOf(envelope.event, 'event payload');
  const type = nonEmptyString(event.type, "event payload 'type'");
  if (!KNOWN_SESSION_EVENT_TYPES.includes(type as KnownSessionEventType)) {
    return { seq: sequence, sessionId, event: { type: 'unknown', eventType: type, raw: event } };
  }
  validateKnownEvent(type as KnownSessionEventType, event);
  return { seq: sequence, sessionId, event: event as EventKindBase };
}

function validateKnownEvent(type: KnownSessionEventType, event: Record<string, unknown>): void {
  switch (type) {
    case 'session_created':
    case 'turn_started':
    case 'turn_completed':
    case 'terminal_attached':
    case 'terminal_detached':
      return;
    case 'progress':
      oneOf(
        event.stage,
        [
          'accepted',
          'runtime_requested',
          'runtime_created',
          'skills_installed',
          'attaching',
          'ready',
        ],
        'progress stage',
      );
      return;
    case 'status_changed':
      nonEmptyString(event.status, 'status_changed status');
      optionalString(event.stop_reason, 'status_changed stop_reason');
      optionalString(event.detail, 'status_changed detail');
      optionalString(event.capability, 'status_changed capability');
      return;
    case 'terminal_resized':
      dimension(event.columns, 'terminal_resized columns');
      dimension(event.rows, 'terminal_resized rows');
      return;
    case 'conversation_opened':
      nonEmptyString(event.conversation_id, 'conversation_opened conversation_id');
      return;
    case 'message_committed':
    case 'owner_message':
      requiredString(event.content, `${type} content`);
      return;
    case 'tool_call_started':
      nonEmptyString(event.call_id, 'tool_call_started call_id');
      requiredString(event.title, 'tool_call_started title');
      nonEmptyString(event.kind, 'tool_call_started kind');
      optionalToolBody(event.arguments, 'tool_call_started arguments');
      return;
    case 'tool_call_updated':
      nonEmptyString(event.call_id, 'tool_call_updated call_id');
      nonEmptyString(event.status, 'tool_call_updated status');
      optionalToolBody(event.result, 'tool_call_updated result');
      return;
    case 'usage_reported':
      safeInteger(event.input_tokens, 'usage_reported input_tokens');
      safeInteger(event.output_tokens, 'usage_reported output_tokens');
      safeInteger(event.context_used, 'usage_reported context_used');
      optionalSafeInteger(event.context_window, 'usage_reported context_window');
      return;
    case 'context_compacted':
      safeInteger(event.was, 'context_compacted was');
      safeInteger(event.now, 'context_compacted now');
      optionalSafeInteger(event.window, 'context_compacted window');
      return;
    case 'mode_changed':
      nonEmptyString(event.mode, 'mode_changed mode');
      return;
    case 'plan_set':
      objectList(event.entries, 'plan_set entries', (entry) => {
        requiredString(entry.title, 'plan entry title');
        requiredString(entry.status, 'plan entry status');
      });
      return;
    case 'config_options_set':
      objectList(event.options, 'config_options_set options', (entry) => {
        nonEmptyString(entry.id, 'config option id');
        requiredString(entry.name, 'config option name');
        requiredString(entry.kind, 'config option kind');
      });
      return;
    case 'available_commands_set':
      objectList(event.commands, 'available_commands_set commands', (entry) => {
        nonEmptyString(entry.name, 'available command name');
        requiredString(entry.description, 'available command description');
      });
      return;
    case 'conversation_gap':
      safeInteger(event.frames, 'conversation_gap frames');
      return;
    case 'sign_in_required':
      nonEmptyString(event.url, 'sign_in_required url');
      optionalString(event.code, 'sign_in_required code');
      optionalString(event.message, 'sign_in_required message');
      return;
    case 'approval_requested':
      nonEmptyString(event.request_id, 'approval_requested request_id');
      optionalString(event.action, 'approval_requested action');
      optionalString(event.detail, 'approval_requested detail');
      return;
    case 'approval_resolved':
      nonEmptyString(event.request_id, 'approval_resolved request_id');
      if (typeof event.allowed !== 'boolean') {
        throw new ProtocolError('approval_resolved allowed was not boolean');
      }
      return;
    case 'plane_changed':
      oneOf(event.from, ['terminal', 'conversation'], 'plane_changed from');
      oneOf(event.to, ['terminal', 'conversation'], 'plane_changed to');
      nonEmptyString(event.snapshot_at, 'plane_changed snapshot_at');
      return;
    case 'memory_pressure_opened':
      safeInteger(event.used_bytes, 'memory_pressure_opened used_bytes');
      safeInteger(event.limit_bytes, 'memory_pressure_opened limit_bytes');
      safeInteger(event.occupancy_permille, 'memory_pressure_opened occupancy_permille');
      return;
    case 'memory_pressure_closed':
      safeInteger(event.peak_bytes, 'memory_pressure_closed peak_bytes');
      safeInteger(event.limit_bytes, 'memory_pressure_closed limit_bytes');
      safeInteger(event.peak_permille, 'memory_pressure_closed peak_permille');
      safeInteger(event.duration_seconds, 'memory_pressure_closed duration_seconds');
      return;
    case 'runtime_lost':
      nonEmptyString(event.runtime_id, 'runtime_lost runtime_id');
      if (event.cause !== undefined && event.cause !== null) {
        oneOf(
          event.cause,
          ['out_of_memory', 'ended', 'failed', 'displaced', 'unrecognised'],
          'runtime_lost cause',
        );
      }
  }
}

function streamPolicy(options: StreamEventsOptions): StreamPolicy {
  const cursor = options.startingAfter ?? 0;
  if (!Number.isSafeInteger(cursor) || cursor < 0) {
    throw new ValidationError('startingAfter must be a non-negative safe integer');
  }
  const initialBackoffMs = options.initialBackoffMs ?? DEFAULT_INITIAL_BACKOFF_MS;
  const maxBackoffMs = options.maxBackoffMs ?? DEFAULT_MAX_BACKOFF_MS;
  const jitterRatio = options.jitterRatio ?? DEFAULT_JITTER_RATIO;
  const maxEmptyConnections = options.maxEmptyConnections ?? DEFAULT_MAX_EMPTY_CONNECTIONS;
  if (!nonNegativeInteger(initialBackoffMs) || !nonNegativeInteger(maxBackoffMs)) {
    throw new ValidationError('stream backoff values must be non-negative safe integers');
  }
  if (initialBackoffMs > maxBackoffMs) {
    throw new ValidationError('initialBackoffMs cannot exceed maxBackoffMs');
  }
  if (!Number.isFinite(jitterRatio) || jitterRatio < 0 || jitterRatio > 1) {
    throw new ValidationError('jitterRatio must be a finite number from zero through one');
  }
  if (!positiveInteger(maxEmptyConnections)) {
    throw new ValidationError('maxEmptyConnections must be a positive safe integer');
  }
  return {
    initialBackoffMs,
    maxBackoffMs,
    jitterRatio,
    maxEmptyConnections,
    random: options.random ?? Math.random,
    sleep:
      options.sleep ??
      ((milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds))),
  };
}

async function waitBeforeReconnect(
  policy: StreamPolicy,
  failedAttempt: number,
  serverRetryMs: number | undefined,
  signal: AbortSignal | undefined,
): Promise<void> {
  throwIfAborted(signal);
  const exponent = Math.min(Math.max(failedAttempt - 1, 0), 31);
  const base =
    serverRetryMs === undefined
      ? Math.min(policy.initialBackoffMs * 2 ** exponent, policy.maxBackoffMs)
      : Math.min(serverRetryMs, policy.maxBackoffMs);
  const random = policy.random();
  if (!Number.isFinite(random) || random < 0 || random > 1) {
    throw new ValidationError(
      'stream random source must return a finite number from zero through one',
    );
  }
  const spread = (random * 2 - 1) * policy.jitterRatio;
  await abortableDelay(policy.sleep(Math.max(0, Math.round(base * (1 + spread)))), signal);
  throwIfAborted(signal);
}

async function abortableDelay(
  delay: Promise<void>,
  signal: AbortSignal | undefined,
): Promise<void> {
  if (signal === undefined) {
    await delay;
    return;
  }
  throwIfAborted(signal);
  let onAbort: (() => void) | undefined;
  const aborted = new Promise<never>((_resolve, reject) => {
    onAbort = () => reject(new RequestAborted(undefined, { cause: signal.reason }));
    signal.addEventListener('abort', onAbort, { once: true });
  });
  try {
    await Promise.race([delay, aborted]);
  } finally {
    if (onAbort !== undefined) {
      signal.removeEventListener('abort', onAbort);
    }
  }
}

function retryable(cause: unknown): boolean {
  return (
    cause instanceof TransportError ||
    cause instanceof TimeoutError ||
    (cause instanceof ApiError && cause.isRetryable)
  );
}

function throwIfAborted(signal: AbortSignal | undefined): void {
  if (signal?.aborted === true) {
    throw new RequestAborted(undefined, { cause: signal.reason });
  }
}

function positiveSequence(raw: string | undefined): number {
  if (raw === undefined || !/^[1-9]\d*$/.test(raw)) {
    throw new ProtocolError(`event stream frame id '${raw ?? ''}' is not a positive sequence`);
  }
  const value = Number(raw);
  if (!Number.isSafeInteger(value)) {
    throw new ProtocolError(
      `event stream frame id '${raw}' exceeds JavaScript's safe integer range`,
    );
  }
  return value;
}

function objectOf(raw: unknown, name: string): Record<string, unknown> {
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) {
    throw new ProtocolError(`${name} was not an object`);
  }
  return raw as Record<string, unknown>;
}

function requiredString(raw: unknown, name: string): string {
  if (typeof raw !== 'string') {
    throw new ProtocolError(`${name} was not a string`);
  }
  return raw;
}

function nonEmptyString(raw: unknown, name: string): string {
  const value = requiredString(raw, name);
  if (value === '') {
    throw new ProtocolError(`${name} was empty`);
  }
  return value;
}

function optionalString(raw: unknown, name: string): void {
  if (raw !== undefined && raw !== null && typeof raw !== 'string') {
    throw new ProtocolError(`${name} was not a string or null`);
  }
}

function safeInteger(raw: unknown, name: string): number {
  if (typeof raw !== 'number' || !Number.isSafeInteger(raw) || raw < 0) {
    throw new ProtocolError(`${name} was not a non-negative safe integer`);
  }
  return raw;
}

function optionalSafeInteger(raw: unknown, name: string): void {
  if (raw !== undefined && raw !== null) {
    safeInteger(raw, name);
  }
}

function dimension(raw: unknown, name: string): void {
  const value = safeInteger(raw, name);
  if (value === 0 || value > 65_535) {
    throw new ProtocolError(`${name} was outside the terminal dimension range`);
  }
}

function oneOf(raw: unknown, values: readonly string[], name: string): void {
  if (typeof raw !== 'string' || !values.includes(raw)) {
    throw new ProtocolError(`${name} carried an unrecognised value`);
  }
}

function optionalToolBody(raw: unknown, name: string): void {
  if (raw === undefined || raw === null) {
    return;
  }
  const value = objectOf(raw, name);
  requiredString(value.text, `${name} text`);
  if (value.truncated !== undefined && typeof value.truncated !== 'boolean') {
    throw new ProtocolError(`${name} truncated was not boolean`);
  }
}

function objectList(
  raw: unknown,
  name: string,
  validate: (entry: Record<string, unknown>) => void,
): void {
  if (!Array.isArray(raw)) {
    throw new ProtocolError(`${name} was not a list`);
  }
  for (const item of raw) {
    validate(objectOf(item, `${name} entry`));
  }
}

function positiveInteger(value: number): boolean {
  return Number.isSafeInteger(value) && value > 0;
}

function nonNegativeInteger(value: number): boolean {
  return Number.isSafeInteger(value) && value >= 0;
}

function messageOf(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}
