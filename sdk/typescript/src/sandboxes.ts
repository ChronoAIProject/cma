import type { ResolvedConfig } from './config.js';
import type { OperationOptions } from './http.js';
import { ProtocolError } from './errors.js';
import { array } from './schema.js';
import {
  command,
  identity,
  input,
  page,
  query,
  read,
  segment,
  type CommandOptions,
  type EventOptions,
  type PageOptions,
} from './resources.js';
import {
  createSandboxRequest,
  grantSandboxServiceAccessRequest,
  sandbox,
  sandboxDetail,
  sandboxStatus,
  sandboxSummary,
  type CreateSandboxRequest,
  type GrantSandboxServiceAccessRequest,
  type SandboxStatus,
} from './instances/types.js';
import { sandboxEvent } from './instances/events.js';

export interface SandboxListOptions extends PageOptions {
  readonly status?: SandboxStatus;
}

/** Owner-scoped Sandbox provisioning, hierarchy reads, and lifecycle commands. */
export class Sandboxes {
  constructor(private readonly config: ResolvedConfig) {}

  list(options: SandboxListOptions = {}) {
    if (options.status !== undefined) input(sandboxStatus, options.status);
    return read(
      this.config,
      query('/api/v1/sandboxes', {
        cursor: options.cursor,
        limit: options.limit,
        status: options.status,
      }),
      page(sandboxSummary),
      options,
    );
  }

  read(id: string, options: OperationOptions = {}) {
    return read(
      this.config,
      `/api/v1/sandboxes/${segment(id)}`,
      identity(sandboxDetail, id),
      options,
    );
  }

  create(body: CreateSandboxRequest, options: CommandOptions) {
    return read(this.config, '/api/v1/sandboxes', sandbox, {
      ...command(options),
      method: 'POST',
      body: input(createSandboxRequest, body),
      expectedStatus: 201,
    });
  }

  delete(id: string, options: CommandOptions) {
    return this.lifecycle(id, '', 'DELETE', options);
  }
  pause(id: string, options: CommandOptions) {
    return this.lifecycle(id, '/pause', 'POST', options);
  }
  revive(id: string, options: CommandOptions) {
    return this.lifecycle(id, '/revive', 'POST', options);
  }

  grantServiceAccess(id: string, body: GrantSandboxServiceAccessRequest, options: CommandOptions) {
    return read(
      this.config,
      `/api/v1/sandboxes/${segment(id)}/service-access`,
      identity(sandbox, id),
      {
        ...command(options),
        method: 'POST',
        body: input(grantSandboxServiceAccessRequest, body),
        expectedStatus: 200,
      },
    );
  }

  events(id: string, options: EventOptions = {}) {
    return read(
      this.config,
      query(`/api/v1/sandboxes/${segment(id)}/events`, {
        after: options.after,
        limit: options.limit,
      }),
      array((value) => {
        const event = sandboxEvent(value);
        if (event.sandbox_id !== id)
          throw new ProtocolError('event Sandbox identity did not match the request');
        return event;
      }),
      options,
    );
  }

  private lifecycle(id: string, suffix: string, method: string, options: CommandOptions) {
    return read(this.config, `/api/v1/sandboxes/${segment(id)}${suffix}`, identity(sandbox, id), {
      ...command(options),
      method,
      expectedStatus: 202,
    });
  }
}
