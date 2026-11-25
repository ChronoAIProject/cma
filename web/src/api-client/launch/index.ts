import { createBoundLaunch, type BoundCreateLaunchOptions } from './subject-binding.js';
import type { ResolvedConfig } from '../config.js';
import { ApiError, ProtocolError } from '../errors.js';
import { readTriggerPreview } from '../triggers.js';
import type { OperationOptions } from '../http.js';
import {
  command,
  identity,
  input,
  query,
  read,
  segment,
  type CommandOptions,
  type EventOptions,
  type PageOptions,
} from '../resources.js';
import { array, boolean, integer, object, optional, text } from '../schema.js';
import { launchEvent } from './events.js';
import { streamLaunchEvents, type LaunchStreamOptions } from './stream.js';
import {
  createLaunchRequest,
  createdServiceConnection,
  createServiceConnectionRequest,
  existingLaunchAgent,
  launch,
  launchConnectionRequirements,
  launchState,
  serviceConnectionStatus,
  startLaunchRequest,
  type CreateLaunchRequest,
  type CreateServiceConnectionRequest,
  type LaunchState,
  type StartLaunchRequest,
} from './types.js';

export type * from './types.js';
export { LaunchSubjectUnverifiedError, type BoundCreateLaunchOptions } from './subject-binding.js';
export type * from './events.js';
export type { LaunchStreamOptions } from './stream.js';

export interface LaunchListOptions extends PageOptions {
  readonly trigger?: string;
  readonly state?: LaunchState;
}

export function repeatChoiceFrom(error: unknown) {
  if (!(error instanceof ApiError) || error.code !== 'trigger_repeat_choice_required')
    return undefined;
  return existingLaunchAgent(error.extensions.existing);
}

/** Visitor admission and owner-only progress. Every mutation requires an explicit key. */
export class Launches {
  constructor(private readonly config: ResolvedConfig) {}

  preview(publicId: string, options: OperationOptions = {}) {
    return readTriggerPreview(this.config, publicId, options);
  }
  entryIntent(publicId: string, options: OperationOptions = {}) {
    return read(
      { ...this.config, credential: undefined },
      `/api/v1/triggers/public/${segment(publicId)}/entry-intent`,
      object({ accepted: boolean, placement_id: optional(text) }),
      { ...options, method: 'POST', expectedStatus: 200 },
    );
  }
  createOrResume(publicId: string, body: CreateLaunchRequest, options: CommandOptions) {
    return read(this.config, `/api/v1/triggers/public/${segment(publicId)}/launches`, launch, {
      ...command(options),
      method: 'POST',
      body: input(createLaunchRequest, body),
      expectedStatus: [201, 202],
    });
  }
  createOrResumeBound(
    publicId: string,
    body: CreateLaunchRequest,
    options: BoundCreateLaunchOptions,
  ) {
    return createBoundLaunch(this.config, publicId, body, options);
  }
  read(id: string, options: OperationOptions = {}) {
    return read(this.config, `/api/v1/launches/${segment(id)}`, identity(launch, id), options);
  }
  async list(options: LaunchListOptions = {}) {
    if (options.state !== undefined) input(launchState, options.state);
    const result = await read(
      this.config,
      query('/api/v1/launches', {
        trigger: options.trigger,
        state: options.state,
        cursor: options.cursor,
        limit: options.limit,
      }),
      object({ items: array(launch), next_cursor: optional(text) }),
      options,
    );
    return { items: result.items, next: result.next_cursor };
  }
  start(id: string, body: StartLaunchRequest, options: CommandOptions) {
    return read(this.config, `/api/v1/launches/${segment(id)}/start`, identity(launch, id), {
      ...command(options),
      method: 'POST',
      body: input(startLaunchRequest, body),
      expectedStatus: 202,
    });
  }
  cancel(id: string, options: CommandOptions) {
    return read(this.config, `/api/v1/launches/${segment(id)}/cancel`, identity(launch, id), {
      ...command(options),
      method: 'POST',
      expectedStatus: 200,
    });
  }
  requirements(id: string, options: OperationOptions = {}) {
    return read(
      this.config,
      `/api/v1/launches/${segment(id)}/requirements`,
      launchConnectionRequirements,
      options,
    );
  }
  async events(id: string, options: EventOptions = {}) {
    const page = await read(
      this.config,
      query(`/api/v1/launches/${segment(id)}/events`, {
        after: options.after,
        limit: options.limit,
      }),
      object({ launch_id: text, events: array(launchEvent), next_cursor: integer }),
      options,
    );
    let cursor = options.after ?? 0;
    for (const event of page.events) {
      if (event.launch_id !== id || event.seq <= cursor)
        throw new ProtocolError('Launch event identity or ordering did not match the request');
      cursor = event.seq;
    }
    if (page.launch_id !== id || page.next_cursor !== cursor)
      throw new ProtocolError('Launch event cursor did not match its page');
    return page;
  }
  stream(id: string, options: LaunchStreamOptions) {
    return streamLaunchEvents(this.config, id, options);
  }
  createConnection(body: CreateServiceConnectionRequest, options: CommandOptions) {
    return read(this.config, '/api/v1/service-connections', createdServiceConnection, {
      ...command(options),
      method: 'POST',
      body: input(createServiceConnectionRequest, body),
      expectedStatus: 201,
    });
  }
  readConnection(id: string, options: OperationOptions = {}) {
    return read(
      this.config,
      `/api/v1/service-connections/${segment(id)}`,
      (value) => {
        const status = serviceConnectionStatus(value);
        if (status.handoff_id !== id)
          throw new ProtocolError('Service connection identity did not match the request');
        return status;
      },
      options,
    );
  }
}
