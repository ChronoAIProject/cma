import type { ResolvedConfig } from './config.js';
import { ProtocolError, ValidationError } from './errors.js';
import type { OperationOptions } from './http.js';
import { query, read, segment, type PageOptions } from './resources.js';
import {
  array,
  boolean,
  enumeration,
  integer,
  nullable,
  object,
  text,
  type Output,
} from './schema.js';

const quantity = (value: unknown): number => {
  if (typeof value !== 'number' || !Number.isFinite(value) || value < 0)
    throw new ProtocolError('expected a nonnegative usage quantity');
  return value;
};
const totals = object({
  uptime_seconds: integer,
  cpu_core_hours: quantity,
  memory_gib_hours: quantity,
  disk_gib_hours: quantity,
  cpu_millis_seconds: integer,
  memory_mib_seconds: integer,
  disk_gib_seconds: integer,
});
const allocation = object({
  running: boolean,
  cpu_millis: integer,
  memory_mib: integer,
  disk_gib: integer,
});
const sandbox = object({
  sandbox_id: text,
  owner: text,
  name: text,
  status: text,
  allocation,
  created_at: integer,
  tracking_started_at: integer,
  as_of: integer,
  complete_lifecycle: boolean,
  totals,
});
const summary = object({
  as_of: integer,
  measurement: enumeration('allocated_resource_time'),
  sandbox_count: integer,
  running_count: integer,
  incomplete_lifecycle_count: integer,
  tracking_started_at: nullable(integer),
  totals,
});
const page = object({ items: array(sandbox), next: nullable(text), as_of: integer });
const event = object({
  transaction_id: text,
  revision: integer,
  source_seq: integer,
  kind: text,
  started_at: integer,
  ended_at: integer,
  status: text,
  allocation,
  next_allocation: allocation,
  quantity: totals,
});
const events = object({ items: array(event), next: nullable(text) });
export type SandboxUsageView = Output<typeof sandbox>;
export type SandboxUsageSummary = Output<typeof summary>;
export type SandboxUsagePage = Output<typeof page>;
export type SandboxUsageEvent = Output<typeof event>;
export type SandboxUsageEvents = Output<typeof events>;
export type SandboxUsageTotals = Output<typeof totals>;
export interface UsagePageOptions extends PageOptions {
  readonly owner?: string;
}

function paging(options: PageOptions) {
  if (
    options.limit !== undefined &&
    (!Number.isInteger(options.limit) || options.limit < 1 || options.limit > 200)
  )
    throw new ValidationError('limit must be between 1 and 200');
  if (options.cursor !== undefined && (options.cursor === '' || options.cursor.length > 256))
    throw new ValidationError('invalid usage cursor');
  return { limit: options.limit, cursor: options.cursor };
}

/** Durable allocated CPU/RAM/disk time; independent of Agent token usage. */
export class SandboxUsage {
  constructor(readonly config: ResolvedConfig) {}
  summary(options: OperationOptions = {}): Promise<SandboxUsageSummary> {
    return read(this.config, '/api/v1/usage', summary, options);
  }
  sandboxes(options: PageOptions = {}): Promise<SandboxUsagePage> {
    return read(this.config, query('/api/v1/usage/sandboxes', paging(options)), page, options);
  }
  sandbox(id: string, options: OperationOptions = {}): Promise<SandboxUsageView> {
    return read(
      this.config,
      `/api/v1/usage/sandboxes/${segment(id)}`,
      (value) => {
        const result = sandbox(value);
        if (result.sandbox_id !== id)
          throw new ProtocolError('usage Sandbox identity did not match');
        return result;
      },
      options,
    );
  }
  events(id: string, options: PageOptions = {}): Promise<SandboxUsageEvents> {
    return read(
      this.config,
      query(`/api/v1/usage/sandboxes/${segment(id)}/events`, paging(options)),
      events,
      options,
    );
  }
  adminSummary(
    options: OperationOptions & { readonly owner?: string } = {},
  ): Promise<SandboxUsageSummary> {
    return read(
      this.config,
      query('/api/v1/admin/usage', { owner: options.owner }),
      summary,
      options,
    );
  }
  adminSandboxes(options: UsagePageOptions = {}): Promise<SandboxUsagePage> {
    return read(
      this.config,
      query('/api/v1/admin/usage/sandboxes', { ...paging(options), owner: options.owner }),
      page,
      options,
    );
  }
}
