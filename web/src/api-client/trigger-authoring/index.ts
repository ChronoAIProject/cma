import type { ResolvedConfig } from '../config.js';
import { ProtocolError } from '../errors.js';
import { request, type OperationOptions } from '../http.js';
import {
  command,
  identity,
  input,
  query,
  read,
  segment,
  type CommandOptions,
  type PageOptions,
  type ResourcePage,
} from '../resources.js';
import { array, object, optional, text } from '../schema.js';
import { readEmbedScript, readTriggerBadge, type TriggerBadgeOptions } from './assets.js';
import {
  createPlacementBody,
  createTriggerBody,
  publishedTriggerRevision,
  publishTriggerBody,
  saveTriggerBody,
  triggerDocument,
  triggerEmbed,
  triggerPlacement,
  triggerRevision,
  triggerStatistics,
  type TriggerContentInput,
  type TriggerDocument,
} from './types.js';

export * from './types.js';
export * from './assets.js';

/** Publisher-owned drafts, immutable publications, placements, and copy-ready embeds. */
export class TriggerAuthoring {
  constructor(private readonly config: ResolvedConfig) {}

  async list(options: PageOptions = {}): Promise<ResourcePage<TriggerDocument>> {
    const result = await read(
      this.config,
      query('/api/v1/triggers', { cursor: options.cursor, limit: options.limit }),
      object({ items: array(triggerDocument), next_cursor: optional(text) }),
      options,
    );
    return {
      items: result.items,
      ...(result.next_cursor === undefined ? {} : { next: result.next_cursor }),
    };
  }

  read(id: string, options: OperationOptions = {}) {
    return read(this.config, this.path(id), identity(triggerDocument, id), options);
  }

  create(body: { readonly content: TriggerContentInput }, options: CommandOptions) {
    return read(this.config, '/api/v1/triggers', triggerDocument, {
      ...command(options),
      method: 'POST',
      body: input<{ readonly content: TriggerContentInput }>(createTriggerBody, body),
      expectedStatus: 201,
    });
  }

  save(
    id: string,
    body: { readonly content: TriggerContentInput; readonly draft_version: number },
    options: CommandOptions,
  ) {
    return read(this.config, this.path(id), identity(triggerDocument, id), {
      ...command(options),
      method: 'PUT',
      body: input<{ readonly content: TriggerContentInput; readonly draft_version: number }>(
        saveTriggerBody,
        body,
      ),
    });
  }

  async delete(id: string, options: CommandOptions): Promise<void> {
    await request(this.config, this.path(id), {
      ...command(options),
      method: 'DELETE',
      expectedStatus: 204,
    });
  }

  publish(id: string, body: { readonly draft_version: number }, options: CommandOptions) {
    return read(
      this.config,
      `${this.path(id)}/publish`,
      (value) => {
        const revision = publishedTriggerRevision(value);
        if (revision.trigger_id !== id)
          throw new ProtocolError('published trigger identity did not match the request');
        return revision;
      },
      {
        ...command(options),
        method: 'POST',
        body: input(publishTriggerBody, body),
        expectedStatus: 201,
      },
    );
  }

  retire(id: string, options: CommandOptions) {
    return read(this.config, `${this.path(id)}/retire`, identity(triggerDocument, id), {
      ...command(options),
      method: 'POST',
    });
  }

  listRevisions(id: string, options: OperationOptions = {}) {
    return read(this.config, `${this.path(id)}/revisions`, array(triggerRevision), options);
  }

  listPlacements(
    id: string,
    options: OperationOptions & { readonly includeRetired?: boolean } = {},
  ) {
    return read(
      this.config,
      query(`${this.path(id)}/placements`, {
        include_retired:
          options.includeRetired === undefined ? undefined : String(options.includeRetired),
      }),
      array(triggerPlacement),
      options,
    );
  }

  createPlacement(
    id: string,
    body: { readonly source_url: string; readonly label: string },
    options: CommandOptions,
  ) {
    return read(this.config, `${this.path(id)}/placements`, triggerPlacement, {
      ...command(options),
      method: 'POST',
      body: input(createPlacementBody, body),
      expectedStatus: 201,
    });
  }

  retirePlacement(id: string, placementId: string, options: CommandOptions) {
    return read(
      this.config,
      `${this.path(id)}/placements/${segment(placementId)}/retire`,
      identity(triggerPlacement, placementId),
      { ...command(options), method: 'POST' },
    );
  }

  embed(id: string, placementId: string, options: OperationOptions = {}) {
    return read(
      this.config,
      query(`${this.path(id)}/embed`, { placement_id: placementId }),
      triggerEmbed,
      options,
    );
  }

  stats(id: string, options: OperationOptions = {}) {
    return read(this.config, `${this.path(id)}/stats`, triggerStatistics, options);
  }

  badge(publicId: string, options: TriggerBadgeOptions = {}) {
    return readTriggerBadge(this.config, publicId, options);
  }

  embedScript(options: OperationOptions = {}) {
    return readEmbedScript(this.config, options);
  }

  private path(id: string): string {
    return `/api/v1/triggers/${segment(id)}`;
  }
}
