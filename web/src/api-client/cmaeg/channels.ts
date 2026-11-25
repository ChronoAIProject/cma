import type { ResolvedConfig } from '../config.js';
import { ValidationError } from '../errors.js';
import { requestVoid, type OperationOptions } from '../http.js';
import { input, query, read, segment } from '../resources.js';
import { ownerHeaders } from './owner.js';
import {
  ChannelPageSchema,
  PartitionInspectionSchema,
  ChannelCreateRequestSchema,
  ChannelUpdateRequestSchema,
  ChannelResponseSchema,
  ChannelRecordManagementSchema,
  TestChannelResponseSchema,
  RekeyResponseSchema,
  SourceCheckResponseSchema,
  RoutingUpdateRequestSchema,
  type RoutingUpdateRequest,
  type ChannelCreateRequest,
  type ChannelUpdateRequest,
} from './types.js';

export interface CreateChannelOptions extends OperationOptions {
  /** Replica-local replay key; reserve a record_id in the body for durable recovery. */
  readonly idempotencyKey?: string;
}

export interface ListChannelOptions extends OperationOptions {
  /** Opaque owner-scoped cursor returned by the previous page. */
  readonly after?: string;
}

export interface ChannelRecordManagementOptions extends OperationOptions {
  /** Organization subject. Omit it to inspect the authenticated creator's records. */
  readonly owner?: string;
}

export class CmaegChannels {
  constructor(private readonly config: ResolvedConfig) {}

  createChannel(body: ChannelCreateRequest, options: CreateChannelOptions = {}) {
    const key = options.idempotencyKey;
    if (key !== undefined && !/^[\x21-\x7e]{1,256}$/.test(key))
      throw new ValidationError('idempotencyKey must contain 1–256 visible ASCII bytes');
    return read(this.config, '/v1/channels', ChannelResponseSchema, {
      ...options,
      method: 'POST',
      expectedStatus: 201,
      body: input(ChannelCreateRequestSchema, body),
      headers: key === undefined ? {} : { 'Idempotency-Key': key },
    });
  }

  listChannels(options: ListChannelOptions = {}) {
    return read(
      this.config,
      query('/v1/channels', { after: options.after }),
      ChannelPageSchema,
      options,
    );
  }

  getChannel(channelId: string, options: OperationOptions = {}) {
    return read(this.config, `/v1/channels/${segment(channelId)}`, ChannelResponseSchema, options);
  }

  inspectChannelPartition(
    channelId: string,
    partitionKey: string,
    options: OperationOptions & { readonly consumerId?: string } = {},
  ) {
    return read(
      this.config,
      query(`/v1/channels/${segment(channelId)}/partition`, {
        partition_key: partitionKey,
        consumer_id: options.consumerId,
      }),
      PartitionInspectionSchema,
      options,
    );
  }

  recoverChannel(recordId: string, options: OperationOptions = {}) {
    return read(
      this.config,
      `/v1/channel-records/${segment(recordId)}`,
      ChannelResponseSchema,
      options,
    );
  }

  getChannelRecordManagement(recordId: string, options: ChannelRecordManagementOptions = {}) {
    return read(
      this.config,
      `/v1/channel-records/${segment(recordId)}/management`,
      ChannelRecordManagementSchema,
      { ...options, headers: ownerHeaders(options.owner) },
    );
  }

  retireLegacyChannelRecord(
    recordId: string,
    expectedVersion: number,
    options: ChannelRecordManagementOptions = {},
  ) {
    if (!Number.isSafeInteger(expectedVersion) || expectedVersion < 1)
      throw new ValidationError('expectedVersion must be a positive safe integer');
    return requestVoid(
      this.config,
      query(`/v1/channel-records/${segment(recordId)}/management`, {
        expected_version: expectedVersion,
      }),
      { ...options, method: 'DELETE', expectedStatus: 204, headers: ownerHeaders(options.owner) },
    );
  }

  updateChannel(channelId: string, body: ChannelUpdateRequest, options: OperationOptions = {}) {
    return read(this.config, `/v1/channels/${segment(channelId)}`, ChannelResponseSchema, {
      ...options,
      method: 'PUT',
      body: input(ChannelUpdateRequestSchema, body),
    });
  }

  deleteChannel(channelId: string, expectedVersion: number, options: OperationOptions = {}) {
    return requestVoid(
      this.config,
      query(`/v1/channels/${segment(channelId)}`, { expected_version: expectedVersion }),
      { ...options, method: 'DELETE', expectedStatus: 204 },
    );
  }

  testChannel(channelId: string, options: OperationOptions = {}) {
    return read(this.config, `/v1/channels/${segment(channelId)}/test`, TestChannelResponseSchema, {
      ...options,
      // Allow the Gateway's 90-second startup and 30-second completion windows.
      timeoutMs: options.timeoutMs ?? 125_000,
      method: 'POST',
    });
  }

  testChannelSource(channelId: string, index: number, options: OperationOptions = {}) {
    if (!Number.isSafeInteger(index) || index < 0)
      throw new ValidationError('source index must be a nonnegative integer');
    return read(
      this.config,
      `/v1/channels/${segment(channelId)}/sources/${index}/test`,
      SourceCheckResponseSchema,
      { ...options, method: 'POST' },
    );
  }
  testChannelConsumer(channelId: string, consumerId: string, options: OperationOptions = {}) {
    return read(
      this.config,
      `/v1/channels/${segment(channelId)}/consumers/${segment(consumerId)}/test`,
      TestChannelResponseSchema,
      { ...options, timeoutMs: options.timeoutMs ?? 125_000, method: 'POST' },
    );
  }
  updateChannelRouting(
    channelId: string,
    body: RoutingUpdateRequest,
    options: OperationOptions = {},
  ) {
    return read(this.config, `/v1/channels/${segment(channelId)}/routing`, ChannelResponseSchema, {
      ...options,
      method: 'PUT',
      body: input(RoutingUpdateRequestSchema, body),
    });
  }

  /** Requires a dedicated CMA service credential, separate from creator custody. */
  rekeyChannel(channelId: string, options: OperationOptions = {}) {
    return read(this.config, `/v1/channels/${segment(channelId)}/rekey`, RekeyResponseSchema, {
      ...options,
      method: 'POST',
    });
  }
}
