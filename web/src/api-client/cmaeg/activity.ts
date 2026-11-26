import { createParser } from 'eventsource-parser';
import type { ResolvedConfig } from '../config.js';
import { ProtocolError, ValidationError } from '../errors.js';
import { raceWithAbortSignal, type OperationOptions } from '../http.js';
import { query, read, segment } from '../resources.js';
import { eventStream } from '../streaming.js';
import { ownerHeaders } from './owner.js';
import {
  ActivityPageSchema,
  HistoryDetailsSchema,
  HistoryPageSchema,
  MediaResponsePageSchema,
  type ActivityPage,
} from './types.js';

export interface ChannelActivityOptions extends OperationOptions {
  /** Organization subject; omitted selects the authenticated creator. */
  readonly owner?: string;
}

/** Use a UTC RFC 3339 range for the first page or the opaque next_cursor to continue. */
export type ChannelActivityHistoryOptions = ChannelActivityOptions & {
  /** Page size from 1 to 200. If repeated, it must match the initial request. */
  readonly limit?: number;
} & (
    | { readonly from: string; readonly until: string; readonly cursor?: never }
    | { readonly cursor: string; readonly from?: string; readonly until?: string }
  );

export interface ChannelActivityHistoryDetailsOptions extends ChannelActivityOptions {
  /** The item's opaque detail_cursor or the previous detail page's next_cursor. */
  readonly cursor: string;
}

export interface EventMediaResponseOptions extends ChannelActivityOptions {
  /** Next attachment-slot offset returned by the previous page. */
  readonly cursor?: number;
}

export interface StreamChannelActivityOptions extends ChannelActivityOptions {
  /** Opaque snapshot watermark, sent as a query parameter. */
  readonly cursor?: string;
  /** Header alternative; takes precedence over cursor, as on the gateway. */
  readonly lastEventId?: string;
}

/** Both event kinds replace the entire recent Activity snapshot atomically. */
export interface ChannelActivityEvent {
  readonly event: 'activity' | 'reset';
  readonly id: string;
  readonly data: ActivityPage;
}

export class CmaegActivity {
  constructor(private readonly config: ResolvedConfig) {}

  getChannelActivity(channelId: string, options: ChannelActivityOptions = {}) {
    return read(this.config, `/v1/channels/${segment(channelId)}/activity`, ActivityPageSchema, {
      ...options,
      headers: ownerHeaders(options.owner),
    });
  }

  getChannelActivityHistory(channelId: string, options: ChannelActivityHistoryOptions) {
    return read(
      this.config,
      query(`/v1/channels/${segment(channelId)}/activity/history`, {
        from: options.from,
        until: options.until,
        limit: options.limit,
        cursor: options.cursor,
      }),
      HistoryPageSchema,
      { ...options, headers: ownerHeaders(options.owner) },
    );
  }

  getChannelActivityHistoryDetails(
    channelId: string,
    historyEventId: string,
    options: ChannelActivityHistoryDetailsOptions,
  ) {
    return read(
      this.config,
      query(
        `/v1/channels/${segment(channelId)}/activity/history/events/${segment(historyEventId)}/details`,
        { cursor: options.cursor },
      ),
      HistoryDetailsSchema,
      { ...options, headers: ownerHeaders(options.owner) },
    );
  }

  /** Metadata only; never follows a Manager response URL or resends the event. */
  getEventMediaResponses(
    channelId: string,
    eventId: string,
    options: EventMediaResponseOptions = {},
  ) {
    if (
      options.cursor !== undefined &&
      (!Number.isSafeInteger(options.cursor) || options.cursor < 0)
    )
      throw new ValidationError('media response cursor must be a nonnegative safe integer');
    return read(
      this.config,
      query(`/v1/channels/${segment(channelId)}/events/${segment(eventId)}/media-responses`, {
        cursor: options.cursor,
      }),
      MediaResponsePageSchema,
      { ...options, headers: ownerHeaders(options.owner) },
    );
  }

  /** Observe snapshots once; reconnect explicitly with the last atomically applied cursor. */
  async *streamChannelActivity(channelId: string, options: StreamChannelActivityOptions = {}) {
    const headers = ownerHeaders(options.owner);
    if (options.lastEventId !== undefined) {
      if (!/^[\x21-\x7e]{1,8192}$/.test(options.lastEventId))
        throw new ValidationError('lastEventId must contain 1–8192 visible ASCII bytes');
      headers['Last-Event-ID'] = options.lastEventId;
    }
    yield* eventStream(
      this.config,
      query(`/v1/channels/${segment(channelId)}/activity/stream`, { cursor: options.cursor }),
      options,
      activityFrames,
      'gateway Activity',
      undefined,
      headers,
    );
  }
}

const MAX_FRAME_BYTES = 8 * 1024 * 1024;

async function* activityFrames(
  response: Response,
  signal: AbortSignal,
): AsyncGenerator<ChannelActivityEvent> {
  if (response.body === null) throw new ProtocolError('Activity stream carried no body');
  const pending: ChannelActivityEvent[] = [];
  let pendingBytes = 0;
  let failure: ProtocolError | undefined;
  let observed = false;
  const parser = createParser({
    maxBufferSize: MAX_FRAME_BYTES,
    onEvent(frame) {
      try {
        pendingBytes += new TextEncoder().encode(frame.data).byteLength;
        if (pendingBytes > 2 * MAX_FRAME_BYTES || pending.length >= 1024)
          throw new ProtocolError('Activity snapshots exceed their pending boundary');
        if (frame.event !== 'activity' && frame.event !== 'reset')
          throw new ProtocolError('unknown Activity stream event');
        const raw: unknown = JSON.parse(frame.data);
        const data = ActivityPageSchema(raw);
        if (data.cursor.length === 0 || frame.id !== data.cursor)
          throw new ProtocolError('Activity snapshot has no matching explicit cursor');
        pending.push({ event: frame.event, id: frame.id, data });
        observed = true;
      } catch (cause) {
        failure = new ProtocolError('invalid Activity stream frame', { cause });
      }
    },
    onError(cause) {
      failure = new ProtocolError('invalid Activity SSE framing', { cause });
    },
  });
  const bound = new ActivityFrameBound();
  const decoder = new TextDecoder('utf-8', { fatal: true });
  const reader = response.body.getReader();
  try {
    while (true) {
      const { done, value } = await raceWithAbortSignal(signal, () => reader.read());
      if (value !== undefined) {
        if (value.byteLength > 4 * 1024 * 1024)
          throw new ProtocolError('Activity transport chunk exceeds its byte boundary');
        bound.inspect(value);
      }
      try {
        parser.feed(done ? decoder.decode() : decoder.decode(value, { stream: true }));
      } catch (cause) {
        throw new ProtocolError('invalid Activity UTF-8 or SSE', { cause });
      }
      if (failure !== undefined) throw failure;
      while (pending.length > 0) yield pending.shift()!;
      pendingBytes = 0;
      if (done) {
        bound.finish();
        if (!observed)
          throw new ProtocolError('Activity stream closed without an initial snapshot');
        return;
      }
    }
  } finally {
    void reader.cancel().catch(() => undefined);
    reader.releaseLock();
  }
}

/** Bound raw frames and reject incomplete snapshots before acknowledging their cursor. */
class ActivityFrameBound {
  private bytes = 0;
  private lineContent = false;
  private previousCr = false;
  private frameContent = false;
  inspect(chunk: Uint8Array) {
    for (const byte of chunk) {
      if (++this.bytes > MAX_FRAME_BYTES)
        throw new ProtocolError('Activity raw frame exceeds its byte boundary');
      if (byte === 10 && this.previousCr) {
        this.previousCr = false;
        continue;
      }
      this.previousCr = byte === 13;
      if (byte === 10 || byte === 13) {
        if (!this.lineContent) {
          this.bytes = 0;
          this.frameContent = false;
        }
        this.lineContent = false;
      } else {
        if (!this.lineContent && byte !== 58) this.frameContent = true;
        this.lineContent = true;
      }
    }
  }
  finish() {
    if (this.frameContent) throw new ProtocolError('Activity stream closed inside a frame');
  }
}
