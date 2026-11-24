import type { ResolvedConfig } from '../config.js';
import { ValidationError } from '../errors.js';
import { requestVoid, type OperationOptions } from '../http.js';
import { input, query, read, segment } from '../resources.js';
import { ownerHeaders } from './owner.js';
import {
  CapabilityViewSchema,
  ConnectionPageSchema,
  ConnectionViewSchema,
  CreateConnectionSchema,
  ResourceConnectionListSchema,
  ResourcePlatformSchema,
  type ResourcePlatform,
  type CreateConnection,
} from './types.js';

export interface ConnectionOptions extends OperationOptions {
  /** Organization subject; omitted selects the authenticated creator. */
  readonly owner?: string;
}
export interface ListConnectionsOptions extends ConnectionOptions {
  /** Continue even when an earlier page has no items. */
  readonly after?: string;
}
export interface CreateConnectionOptions extends ConnectionOptions {
  /** Channel agent credential authorizing the request's originating event. */
  readonly agentKey: string;
}

export class CmaegConnections {
  constructor(private readonly config: ResolvedConfig) {}

  getChannelCapabilities(options: OperationOptions = {}) {
    return read(this.config, '/v1/channel-capabilities', CapabilityViewSchema, options);
  }

  /** Current subject's eligible connections; no owner override or scope mutation. */
  listRelayResourceConnections(platform: ResourcePlatform, options: OperationOptions = {}) {
    return read(
      this.config,
      query('/v1/relay-resource-connections', {
        platform: input(ResourcePlatformSchema, platform),
      }),
      ResourceConnectionListSchema,
      options,
    );
  }

  createChannelConnection(body: CreateConnection, options: CreateConnectionOptions) {
    if (!/^[\x21-\x7e]{1,16384}$/.test(options.agentKey))
      throw new ValidationError('agentKey must contain 1–16384 visible ASCII bytes');
    return read(this.config, '/v1/channel-connections', ConnectionViewSchema, {
      ...options,
      method: 'POST',
      expectedStatus: 201,
      body: input(CreateConnectionSchema, body),
      headers: {
        ...ownerHeaders(options.owner),
        'X-CMAEG-Agent-Authorization': `Bearer ${options.agentKey}`,
      },
    });
  }

  listChannelConnections(options: ListConnectionsOptions = {}) {
    return read(
      this.config,
      query('/v1/channel-connections', { after: options.after }),
      ConnectionPageSchema,
      { ...options, headers: ownerHeaders(options.owner) },
    );
  }

  getChannelConnection(ref: string, options: ConnectionOptions = {}) {
    return read(this.config, `/v1/channel-connections/${segment(ref)}`, ConnectionViewSchema, {
      ...options,
      headers: ownerHeaders(options.owner),
    });
  }

  cancelChannelConnection(ref: string, options: ConnectionOptions = {}) {
    return requestVoid(this.config, `/v1/channel-connections/${segment(ref)}`, {
      ...options,
      method: 'DELETE',
      expectedStatus: 204,
      headers: ownerHeaders(options.owner),
    });
  }
}
