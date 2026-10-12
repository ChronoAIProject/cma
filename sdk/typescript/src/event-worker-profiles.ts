import type { ResolvedConfig } from './config.js';
import type { OperationOptions } from './http.js';
import { command, input, query, read, segment, type CommandOptions } from './resources.js';
import { array, nullable, object, positiveInteger, text, type Output } from './schema.js';
import { EventWorkerProfileSchema } from './cmaeg/types/profile.js';

const reference = object({ id: text, revision: positiveInteger });
const source = object({
  name: text,
  description: text,
  agent_profile: reference,
  workspace_profile: reference,
  sandbox_profile: reference,
});
const profile = object({
  id: text,
  owner: text,
  revision: positiveInteger,
  source,
  profile: EventWorkerProfileSchema,
  digest: text,
  created_at: text,
  updated_at: text,
  retired_at: nullable(text),
});
export type EventWorkerProfileRecord = Output<typeof profile>;
export type EventWorkerProfileInput = Output<typeof source>;
export interface EventWorkerProfileListOptions extends OperationOptions {
  readonly after?: string;
  readonly limit?: number;
}
const base = '/api/v1/event-worker-profiles';
/** Each save creates an immutable revision. Existing Bots retain their selected snapshot. */
export class EventWorkerProfiles {
  constructor(private readonly config: ResolvedConfig) {}
  list(options: EventWorkerProfileListOptions = {}) {
    return read(
      this.config,
      query(base, { after: options.after, limit: options.limit }),
      object({ items: array(profile), next_after: nullable(text) }),
      options,
    );
  }
  get(id: string, options: OperationOptions = {}) {
    return read(this.config, `${base}/${segment(id)}`, profile, options);
  }
  revision(id: string, revision: number, options: OperationOptions = {}) {
    return read(
      this.config,
      `${base}/${segment(id)}/revisions/${positiveInteger(revision)}`,
      profile,
      options,
    );
  }
  create(body: EventWorkerProfileInput, options: CommandOptions) {
    return read(this.config, base, profile, {
      ...command(options),
      method: 'POST',
      expectedStatus: 201,
      body: input(source, body),
    });
  }
  update(
    id: string,
    expectedRevision: number,
    body: EventWorkerProfileInput,
    options: OperationOptions = {},
  ) {
    return read(this.config, `${base}/${segment(id)}`, profile, {
      ...options,
      method: 'PUT',
      body: { expected_revision: positiveInteger(expectedRevision), source: input(source, body) },
    });
  }
  retire(id: string, expectedRevision: number, options: OperationOptions = {}) {
    return read(
      this.config,
      query(`${base}/${segment(id)}`, { expected_revision: positiveInteger(expectedRevision) }),
      profile,
      { ...options, method: 'DELETE' },
    );
  }
}
