import { CmaegOutbound, type PrepareOutboundOptions } from './outbound.js';
import type { PrepareOutboundMessage } from './types.js';
import { resolveConfig, type ClientConfig, type ResolvedConfig } from '../config.js';
import type { OperationOptions } from '../http.js';
import {
  CmaegChannels,
  type ChannelRecordManagementOptions,
  type CreateChannelOptions,
  type ListChannelOptions,
} from './channels.js';
import {
  CmaegConnections,
  type ConnectionOptions,
  type CreateConnectionOptions,
  type ListConnectionsOptions,
} from './connections.js';
import { CmaegEvents, type EventOptions, type StreamEventOptions } from './events.js';
import type { GatewayMediaStreamEvent, GatewayStreamEvent } from './stream.js';
import { CmaegTools } from './tools.js';
import type { CreateThreadRequest, ManageThreadRequest, ThreadListView } from './types.js';
import {
  CmaegActivity,
  type ChannelActivityHistoryDetailsOptions,
  type ChannelActivityHistoryOptions,
  type ChannelActivityOptions,
  type EventMediaResponseOptions,
  type StreamChannelActivityOptions,
} from './activity.js';
import type {
  ChannelCreateRequest,
  ChannelUpdateRequest,
  CloudEvent,
  CreateConnection,
  ResourcePlatform,
  OutboundMessage,
  RoutingUpdateRequest,
  MessageReactionRequest,
} from './types.js';

/** Independent gateway deployment configuration; baseUrl names the origin, without /v1. */
export interface CmaegConfig extends ClientConfig {
  readonly baseUrl: string;
}

/** CMAEG management, custom event and tool operations. No mutation is retried automatically. */
export class Cmaeg {
  readonly #channels: CmaegChannels;
  readonly #activity: CmaegActivity;
  readonly #outbound: CmaegOutbound;
  readonly #connections: CmaegConnections;
  readonly #events: CmaegEvents;
  readonly #tools: CmaegTools;
  constructor(config: CmaegConfig) {
    const resolved = resolveConfig(config);
    const transport: ResolvedConfig = {
      ...resolved,
      fetch: (input, init) => resolved.fetch(input, { ...init, redirect: 'manual' }),
    };
    this.#channels = new CmaegChannels(transport);
    this.#activity = new CmaegActivity(transport);
    this.#outbound = new CmaegOutbound(transport);
    this.#connections = new CmaegConnections(transport);
    this.#events = new CmaegEvents(transport);
    this.#tools = new CmaegTools(transport);
  }
  prepareOutboundMessage(
    channel: string,
    body: PrepareOutboundMessage,
    options: PrepareOutboundOptions,
  ) {
    return this.#outbound.prepareOutboundMessage(channel, body, options);
  }
  getOutboundMessage(channel: string, delivery: string, options?: OperationOptions) {
    return this.#outbound.getOutboundMessage(channel, delivery, options);
  }
  cancelOutboundMessage(channel: string, delivery: string, options?: OperationOptions) {
    return this.#outbound.cancelOutboundMessage(channel, delivery, options);
  }
  commitOutboundMessage(
    channel: string,
    delivery: string,
    body: OutboundMessage,
    options?: OperationOptions,
  ) {
    return this.#outbound.commitOutboundMessage(channel, delivery, body, options);
  }
  sendOutboundMessage(
    channel: string,
    body: PrepareOutboundMessage,
    options: PrepareOutboundOptions,
  ) {
    return this.#outbound.sendOutboundMessage(channel, body, options);
  }
  getChannelCapabilities(options?: OperationOptions) {
    return this.#connections.getChannelCapabilities(options);
  }
  listRelayResourceConnections(platform: ResourcePlatform, options?: OperationOptions) {
    return this.#connections.listRelayResourceConnections(platform, options);
  }
  createChannelConnection(body: CreateConnection, options: CreateConnectionOptions) {
    return this.#connections.createChannelConnection(body, options);
  }
  listChannelConnections(options?: ListConnectionsOptions) {
    return this.#connections.listChannelConnections(options);
  }
  getChannelConnection(ref: string, options?: ConnectionOptions) {
    return this.#connections.getChannelConnection(ref, options);
  }
  cancelChannelConnection(ref: string, options?: ConnectionOptions) {
    return this.#connections.cancelChannelConnection(ref, options);
  }
  createChannel(body: ChannelCreateRequest, options?: CreateChannelOptions) {
    return this.#channels.createChannel(body, options);
  }
  listChannels(options?: ListChannelOptions) {
    return this.#channels.listChannels(options);
  }
  getChannel(id: string, options?: OperationOptions) {
    return this.#channels.getChannel(id, options);
  }
  inspectChannelPartition(
    id: string,
    partitionKey: string,
    options?: OperationOptions & { readonly consumerId?: string },
  ) {
    return this.#channels.inspectChannelPartition(id, partitionKey, options);
  }
  getChannelActivity(id: string, options?: ChannelActivityOptions) {
    return this.#activity.getChannelActivity(id, options);
  }
  /** Fetch one page of retained receipts, with coverage for the requested range. */
  getChannelActivityHistory(id: string, options: ChannelActivityHistoryOptions) {
    return this.#activity.getChannelActivityHistory(id, options);
  }
  /** Fetch one page of detail fragments using an item's opaque history_event_id. */
  getChannelActivityHistoryDetails(
    id: string,
    historyEventId: string,
    options: ChannelActivityHistoryDetailsOptions,
  ) {
    return this.#activity.getChannelActivityHistoryDetails(id, historyEventId, options);
  }
  getEventMediaResponses(id: string, eventId: string, options?: EventMediaResponseOptions) {
    return this.#activity.getEventMediaResponses(id, eventId, options);
  }
  streamChannelActivity(id: string, options?: StreamChannelActivityOptions) {
    return this.#activity.streamChannelActivity(id, options);
  }
  recoverChannel(id: string, options?: OperationOptions) {
    return this.#channels.recoverChannel(id, options);
  }
  /** Inspect a retained record using its storage ID and optional organization scope. */
  getChannelRecordManagement(recordId: string, options?: ChannelRecordManagementOptions) {
    return this.#channels.getChannelRecordManagement(recordId, options);
  }
  retireLegacyChannelRecord(
    recordId: string,
    expectedVersion: number,
    options?: ChannelRecordManagementOptions,
  ) {
    return this.#channels.retireLegacyChannelRecord(recordId, expectedVersion, options);
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
  testChannelSource(id: string, index: number, options?: OperationOptions) {
    return this.#channels.testChannelSource(id, index, options);
  }
  testChannelConsumer(id: string, consumerId: string, options?: OperationOptions) {
    return this.#channels.testChannelConsumer(id, consumerId, options);
  }
  updateChannelRouting(id: string, body: RoutingUpdateRequest, options?: OperationOptions) {
    return this.#channels.updateChannelRouting(id, body, options);
  }
  rekeyChannel(id: string, options?: OperationOptions) {
    return this.#channels.rekeyChannel(id, options);
  }
  postEvent(id: string, event: CloudEvent, options?: EventOptions) {
    return this.#events.postEvent(id, event, options);
  }
  streamEvent(
    id: string,
    event: CloudEvent,
    options?: EventOptions & { readonly mediaResponses?: false },
  ): AsyncGenerator<GatewayStreamEvent>;
  streamEvent(
    id: string,
    event: CloudEvent,
    options: StreamEventOptions,
  ): AsyncGenerator<GatewayMediaStreamEvent>;
  streamEvent(id: string, event: CloudEvent, options: StreamEventOptions = {}) {
    return this.#events.streamEvent(id, event, options);
  }
  replyToEvent(
    ref: string,
    message: OutboundMessage,
    options?: OperationOptions & { idempotencyKey?: string; formPreparationRef?: string },
  ) {
    return this.#tools.replyToEvent(ref, message, options);
  }
  prepareFormReply(
    ref: string,
    message: OutboundMessage,
    idempotencyKey: string,
    options?: OperationOptions,
  ) {
    return this.#tools.prepareFormReply(ref, message, idempotencyKey, options);
  }
  readFormPreparation(ref: string, options?: OperationOptions) {
    return this.#tools.readFormPreparation(ref, options);
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
  updateMessage(
    ref: string,
    message: OutboundMessage,
    options?: OperationOptions & { idempotencyKey?: string },
  ) {
    return this.#tools.updateMessage(ref, message, options);
  }
  readMessageCapabilities(ref: string, options?: OperationOptions) {
    return this.#tools.readMessageCapabilities(ref, options);
  }
  deleteMessage(ref: string, idempotencyKey: string, options?: OperationOptions) {
    return this.#tools.deleteMessage(ref, idempotencyKey, options);
  }
  setMessageReaction(
    ref: string,
    idempotencyKey: string,
    request: MessageReactionRequest,
    options?: OperationOptions,
  ) {
    return this.#tools.setMessageReaction(ref, idempotencyKey, request, options);
  }
  sendTyping(ref: string, options?: OperationOptions) {
    return this.#tools.sendTyping(ref, options);
  }
  /** Read one page only; pass the returned cursor explicitly to continue backward. */
  readEventHistory(
    eventRef: string,
    cursor?: string,
    options?: OperationOptions & { includeMedia?: boolean },
  ) {
    return this.#tools.readEventHistory(eventRef, cursor, options);
  }
  readEventContext(ref: string, options?: OperationOptions & { includeThreadHistory?: boolean }) {
    return this.#tools.readEventContext(ref, options);
  }
  /** List only the original native Discord thread scope. */
  listEventThreads(ref: string, view?: ThreadListView, options?: OperationOptions) {
    return this.#tools.listEventThreads(ref, view, options);
  }
  /** Create a public thread anchored to the original human message. */
  createEventThread(
    ref: string,
    key: string,
    request: CreateThreadRequest,
    options?: OperationOptions,
  ) {
    return this.#tools.createEventThread(ref, key, request, options);
  }
  /** Mutate one exact scoped thread; unknown outcomes must not be retried with another key. */
  manageThread(key: string, request: ManageThreadRequest, options?: OperationOptions) {
    return this.#tools.manageThread(key, request, options);
  }
}

export { signWebhook, type WebhookHeaders } from './signing.js';
export type {
  ChannelRecordManagementOptions,
  CreateChannelOptions,
  ListChannelOptions,
} from './channels.js';
export type { EventOptions, StreamEventOptions } from './events.js';
export type { GatewayStreamEvent, GatewayMediaStreamEvent } from './stream.js';
export type {
  EventMediaResponseOptions,
  ChannelActivityHistoryDetailsOptions,
  ChannelActivityHistoryOptions,
  ChannelActivityOptions,
  StreamChannelActivityOptions,
  ChannelActivityEvent,
} from './activity.js';
export type * from './types.js';
export type {
  ConnectionOptions,
  CreateConnectionOptions,
  ListConnectionsOptions,
} from './connections.js';

export { OutboundCommitUncertainError, type PrepareOutboundOptions } from './outbound.js';
