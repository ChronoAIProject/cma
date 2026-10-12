import type { ResolvedConfig } from '../config.js';
import { RequestAborted, TimeoutError, ValidationError } from '../errors.js';
import { requestResponse, type OperationOptions } from '../http.js';
import { input, read, segment } from '../resources.js';
import { CloudEventSchema, EventAcceptedSchema, type CloudEvent } from './types.js';
import { signWebhook } from './signing.js';
import { gatewayFrames } from './stream.js';

export interface EventOptions extends OperationOptions {
  /** Omit for a source authenticated with the configured bearer credential. */
  readonly webhook?: { readonly secret: string; readonly id: string; readonly timestamp: string };
}

async function prepare(event: CloudEvent, options: EventOptions) {
  // Snapshot before signing: caller mutation or a toJSON hook cannot change signed bytes.
  const raw = JSON.stringify(input(CloudEventSchema, event));
  if (new TextEncoder().encode(raw).byteLength > 256 * 1024)
    throw new ValidationError('custom event exceeds 256 KiB');
  const body: unknown = JSON.parse(raw);
  const validated = CloudEventSchema(body);
  const signed =
    options.webhook === undefined
      ? {}
      : await signWebhook(
          options.webhook.secret,
          options.webhook.id,
          options.webhook.timestamp,
          raw,
        );
  return {
    body,
    eventId: validated.id,
    headers: { 'Content-Type': 'application/cloudevents+json', ...signed },
  };
}

export class CmaegEvents {
  constructor(private readonly config: ResolvedConfig) {}

  async postEvent(channelId: string, event: CloudEvent, options: EventOptions = {}) {
    const path = `/v1/channels/${segment(channelId)}/events`;
    const prepared = await prepare(event, options);
    return read(this.eventConfig(options), path, EventAcceptedSchema, {
      ...options,
      ...prepared,
      method: 'POST',
      expectedStatus: 202,
    });
  }

  /** One admitted event, with no automatic reconnect or retry after uncertainty. */
  async *streamEvent(channelId: string, event: CloudEvent, options: EventOptions = {}) {
    const path = `/v1/channels/${segment(channelId)}/events?mode=stream`;
    const prepared = await prepare(event, options);
    const timeout = options.timeoutMs ?? this.config.requestTimeoutMs;
    if (!Number.isSafeInteger(timeout) || timeout <= 0)
      throw new ValidationError('timeoutMs must be a positive safe integer');
    const controller = new AbortController();
    const abort = () => controller.abort(new RequestAborted());
    if (options.signal?.aborted === true) abort();
    options.signal?.addEventListener('abort', abort, { once: true });
    const timer = setTimeout(() => controller.abort(new TimeoutError(timeout)), timeout);
    try {
      const response = await requestResponse(this.eventConfig(options), path, {
        ...options,
        ...prepared,
        method: 'POST',
        expectedStatus: 200,
        headers: { ...prepared.headers, Accept: 'text/event-stream' },
        signal: controller.signal,
      });
      yield* gatewayFrames(response, controller.signal, prepared.eventId);
    } finally {
      clearTimeout(timer);
      options.signal?.removeEventListener('abort', abort);
      controller.abort();
    }
  }

  private eventConfig(options: EventOptions): ResolvedConfig {
    return options.webhook === undefined
      ? this.config
      : { ...this.config, credential: undefined, cookies: false };
  }
}
