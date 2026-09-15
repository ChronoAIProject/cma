import type { ResolvedConfig } from '../config.js';
import { ProtocolError } from '../errors.js';
import type { OperationOptions } from '../http.js';
import { input, query, read, segment, type PageOptions } from '../resources.js';
import { resourceActor } from '../resource-access.js';
import {
  array,
  boolean,
  enumeration,
  nullable,
  object,
  optional,
  positiveInteger,
  tagged,
  text,
  type Output,
} from '../schema.js';

/** Safe channel attribution, separate from immutable creation origin. */
export const channelUsageSummary = object({
  active: boolean,
  has_history: boolean,
  first_used_at: nullable(text),
  last_used_at: nullable(text),
});
export type ChannelUsageSummary = Output<typeof channelUsageSummary>;

/** Management-only binding reference and its recorded accepted-command use. */
const binding = object({
  binding_id: text,
  bridge_id: text,
  partition_id: text,
  generation: positiveInteger,
  previous_binding_id: nullable(text),
  attached_at: text,
  first_used_at: nullable(text),
  last_used_at: nullable(text),
});
export type ChannelBinding = Output<typeof binding>;
const usage = object({
  agent_id: text,
  channel_usage: channelUsageSummary,
  bindings: optional(array(binding)),
});
/** Ordinary shared readers receive only `channel_usage`; managers also receive bindings. */
export type AgentChannelUsage = Output<typeof usage>;
const change = tagged(
  {
    attach: object({ kind: enumeration('attach'), binding_id: text }),
    use: object({ kind: enumeration('use'), binding_id: text }),
    detach: object({ kind: enumeration('detach'), binding_id: text }),
    replace: object({ kind: enumeration('replace'), before_binding_id: text, binding_id: text }),
  },
  'kind',
);
/** Explicit channel binding or accepted-command transition. */
export type ChannelUsageChange = Output<typeof change>;
const reason = enumeration(
  'attachment',
  'detachment',
  'reset',
  'rebind',
  'administrative_intervention',
  'accepted_command',
);
export type ChannelBindingChangeReason = Output<typeof reason>;
const event = object({
  revision: positiveInteger,
  actor: resourceActor,
  at: text,
  detached_at: nullable(text),
  reason,
  change,
  before: array(binding),
  after: array(binding),
});
/** Manager-only immutable history; before/after retain detached binding use. */
export type ChannelUsageEvent = Output<typeof event>;
const history = object({ agent_id: text, items: array(event), next: optional(text) });
export type AgentChannelUsageHistory = Output<typeof history>;

function cursor(value: unknown): number {
  const parsed = text(value);
  const revision = Number(parsed);
  if (!/^[1-9][0-9]*$/.test(parsed) || !Number.isSafeInteger(revision))
    throw new ProtocolError('channel history cursor must be a positive revision');
  return revision;
}
function pairs(first: string | null, last: string | null): boolean {
  return (first === null && last === null) || (first !== null && last !== null && first <= last);
}
function bindings(items: readonly ChannelBinding[]): void {
  if (items.length > 16 || new Set(items.map((item) => item.binding_id)).size !== items.length)
    throw new ProtocolError('channel bindings exceeded their bounds or repeated an identity');
}
function collection(id: string): string {
  return `/api/v1/agents/${segment(id)}/channel-usage`;
}

export function readAgentChannelUsage(
  config: ResolvedConfig,
  id: string,
  options: OperationOptions = {},
) {
  return read(
    config,
    collection(id),
    (value) => {
      const result = usage(value);
      if (result.agent_id !== id)
        throw new ProtocolError('channel usage Agent identity did not match the request');
      const summary = result.channel_usage;
      if (
        !pairs(summary.first_used_at, summary.last_used_at) ||
        (!summary.has_history && (summary.active || summary.first_used_at !== null))
      )
        throw new ProtocolError('channel usage summary is inconsistent');
      if (result.bindings !== undefined) {
        bindings(result.bindings);
        if (result.bindings.length > 0 !== summary.active)
          throw new ProtocolError('channel binding summary disagrees with its active records');
      }
      return result;
    },
    options,
  );
}

export function listAgentChannelUsageHistory(
  config: ResolvedConfig,
  id: string,
  options: PageOptions = {},
) {
  if (options.cursor !== undefined)
    input((text) => {
      cursor(text);
      return text;
    }, options.cursor);
  const after = options.cursor === undefined ? 0 : cursor(options.cursor);
  if (options.limit !== undefined)
    input((value) => {
      const limit = positiveInteger(value);
      if (limit > 200) throw new ProtocolError('channel history limit must be at most 200');
      return limit;
    }, options.limit);
  return read(
    config,
    query(`${collection(id)}/history`, { cursor: options.cursor, limit: options.limit }),
    (value) => {
      const result = history(value);
      if (result.agent_id !== id)
        throw new ProtocolError('channel history Agent identity did not match the request');
      if (result.items.length > (options.limit ?? 50))
        throw new ProtocolError('channel history exceeded its page limit');
      let previous = after;
      for (const event of result.items) {
        if (event.revision !== previous + 1)
          throw new ProtocolError('channel history revisions are not contiguous');
        previous = event.revision;
        bindings(event.before);
        bindings(event.after);
        const detached = event.change.kind === 'detach' || event.change.kind === 'replace';
        if (event.detached_at !== (detached ? event.at : null))
          throw new ProtocolError('channel history detach time is inconsistent');
      }
      if (
        result.next !== undefined &&
        (cursor(result.next) !== previous || result.items.length === 0)
      )
        throw new ProtocolError('channel history continuation did not match its page');
      return result;
    },
    options,
  );
}
