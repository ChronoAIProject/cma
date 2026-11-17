import type { ResolvedConfig } from '../config.js';
import { ProtocolError } from '../errors.js';
import { input, query, read, segment, type PageOptions } from '../resources.js';
import {
  array,
  boolean,
  enumeration,
  object,
  optional,
  positiveInteger,
  text,
  type Output,
} from '../schema.js';

const association = object({
  binding_id: text,
  binding_generation: positiveInteger,
  provider_conversation_id: text,
  relationship: enumeration('managed', 'borrowed'),
  sandbox_id: text,
  workspace_id: text,
  worktree_id: text,
  conversation_state: enumeration('provisioning', 'ready', 'failed', 'retiring', 'retired'),
  binding_state: enumeration('provisioning', 'ready', 'failed', 'retired'),
  current_binding_generation: positiveInteger,
  is_current_generation: boolean,
  association_created_at: text,
  association_updated_at: text,
});
/** Retained membership, including retired generations; timestamps are not event-use history. */
export type ProviderAssociation = Output<typeof association>;
const page = object({
  agent_id: text,
  owner: text,
  agent_created_at: text,
  items: array(association),
  next: optional(text),
});
/** Requires current Agent management authority, never provider worker credentials. */
export type AgentProviderAssociations = Output<typeof page>;

function cursor(value: unknown): string {
  const result = text(value);
  if (result.length > 4096 || !/^[A-Za-z0-9_-]+$/.test(result))
    throw new ProtocolError('provider association cursor must be an opaque continuation');
  return result;
}
export function listAgentProviderAssociations(
  config: ResolvedConfig,
  id: string,
  options: PageOptions = {},
) {
  if (options.cursor !== undefined) input(cursor, options.cursor);
  if (options.limit !== undefined)
    input((value) => {
      const limit = positiveInteger(value);
      if (limit > 200) throw new ProtocolError('provider association limit must be at most 200');
      return limit;
    }, options.limit);
  return read(
    config,
    query(`/api/v1/agents/${segment(id)}/provider-associations`, {
      cursor: options.cursor,
      limit: options.limit,
    }),
    (value) => {
      const result = page(value);
      if (result.agent_id !== id || result.items.length > (options.limit ?? 50))
        throw new ProtocolError('provider association page identity or bound is invalid');
      let previous = '';
      for (const item of result.items) {
        const key = `${item.binding_id}:${String(item.binding_generation).padStart(20, '0')}:${item.provider_conversation_id}`;
        if (
          key <= previous ||
          item.binding_generation > item.current_binding_generation ||
          item.is_current_generation !==
            (item.binding_generation === item.current_binding_generation)
        )
          throw new ProtocolError('provider association identity or generation is inconsistent');
        previous = key;
      }
      if (
        result.next !== undefined &&
        (cursor(result.next) === options.cursor || result.items.length !== (options.limit ?? 50))
      )
        throw new ProtocolError('provider association continuation did not advance a full page');
      return result;
    },
    options,
  );
}
