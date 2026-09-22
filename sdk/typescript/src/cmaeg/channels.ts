import type { ResolvedConfig } from '../config.js';
import { ValidationError } from '../errors.js';
import { requestVoid, type OperationOptions } from '../http.js';
import { input, query, read, segment } from '../resources.js';
import {
  ChannelCreateRequestSchema,
  ChannelUpdateRequestSchema,
  ChannelResponseSchema,
  TestChannelResponseSchema,
  RekeyResponseSchema,
  type ChannelCreateRequest,
  type ChannelUpdateRequest,
} from './types.js';

export interface CreateChannelOptions extends OperationOptions {
  /** Replica-local replay key; reserve a record_id in the body for durable recovery. */
  readonly idempotencyKey?: string;
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

  getChannel(channelId: string, options: OperationOptions = {}) {
    return read(this.config, `/v1/channels/${segment(channelId)}`, ChannelResponseSchema, options);
  }

  recoverChannel(recordId: string, options: OperationOptions = {}) {
    return read(
      this.config,
      `/v1/channel-records/${segment(recordId)}`,
      ChannelResponseSchema,
      options,
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
      method: 'POST',
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
