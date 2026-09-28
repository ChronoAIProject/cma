import { resolveConfig, type ClientConfig, type ResolvedConfig } from '../config.js';
import type { OperationOptions } from '../http.js';
import { CmaegChannels, type CreateChannelOptions } from './channels.js';
import { CmaegEvents, type EventOptions } from './events.js';
import { CmaegTools } from './tools.js';
import type {
  ChannelCreateRequest,
  ChannelUpdateRequest,
  CloudEvent,
  OutboundMessage,
} from './types.js';

/** Independent gateway deployment configuration; baseUrl names the origin, without /v1. */
export interface CmaegConfig extends ClientConfig {
  readonly baseUrl: string;
}

/** CMAEG management, custom event and tool operations. No mutation is retried automatically. */
export class Cmaeg {
  readonly #channels: CmaegChannels;
  readonly #events: CmaegEvents;
  readonly #tools: CmaegTools;
  constructor(config: CmaegConfig) {
    const resolved = resolveConfig(config);
    const transport: ResolvedConfig = {
      ...resolved,
      fetch: (input, init) => resolved.fetch(input, { ...init, redirect: 'manual' }),
    };
    this.#channels = new CmaegChannels(transport);
    this.#events = new CmaegEvents(transport);
    this.#tools = new CmaegTools(transport);
  }
  createChannel(body: ChannelCreateRequest, options?: CreateChannelOptions) {
    return this.#channels.createChannel(body, options);
  }
  getChannel(id: string, options?: OperationOptions) {
    return this.#channels.getChannel(id, options);
  }
  recoverChannel(id: string, options?: OperationOptions) {
    return this.#channels.recoverChannel(id, options);
  }
  updateChannel(id: string, body: ChannelUpdateRequest, options?: OperationOptions) {
    return this.#channels.updateChannel(id, body, options);
  }
  deleteChannel(id: string, expectedVersion: number, options?: OperationOptions) {
    return this.#channels.deleteChannel(id, expectedVersion, options);
  }
  testChannel(id: string, options?: OperationOptions) {
    return this.#channels.testChannel(id, options);
  }
  rekeyChannel(id: string, options?: OperationOptions) {
    return this.#channels.rekeyChannel(id, options);
  }
  postEvent(id: string, event: CloudEvent, options?: EventOptions) {
    return this.#events.postEvent(id, event, options);
  }
  streamEvent(id: string, event: CloudEvent, options?: EventOptions) {
    return this.#events.streamEvent(id, event, options);
  }
  replyToEvent(ref: string, message: OutboundMessage, options?: OperationOptions) {
    return this.#tools.replyToEvent(ref, message, options);
  }
  createReplyTarget(ref: string, options?: OperationOptions) {
    return this.#tools.createReplyTarget(ref, options);
  }
  replyToTarget(
    ref: string,
    idempotencyKey: string,
    message: OutboundMessage,
    options?: OperationOptions,
  ) {
    return this.#tools.replyToTarget(ref, idempotencyKey, message, options);
  }
  updateMessage(ref: string, message: OutboundMessage, options?: OperationOptions) {
    return this.#tools.updateMessage(ref, message, options);
  }
  sendTyping(ref: string, options?: OperationOptions) {
    return this.#tools.sendTyping(ref, options);
  }
  readEventContext(ref: string, options?: OperationOptions) {
    return this.#tools.readEventContext(ref, options);
  }
}

export { signWebhook, type WebhookHeaders } from './signing.js';
export type { CreateChannelOptions } from './channels.js';
export type { EventOptions } from './events.js';
export type { GatewayStreamEvent } from './stream.js';
export type * from './types.js';
