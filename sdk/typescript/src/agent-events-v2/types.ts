import { ProtocolError } from '../errors.js';
import {
  array,
  boolean,
  enumeration,
  integer,
  nullable,
  optional,
  positiveInteger,
  text,
  type Output,
} from '../schema.js';
import { v2AgentEventKind } from './kinds.js';
import {
  cursor,
  cursorIncarnation,
  cursorSequence,
  identifier,
  incarnation,
  pageCoordinates,
  pageCursor,
  strict,
  version,
  type V2AgentPageCursor,
} from './schema.js';

export const v2UiTransport = strict({
  object: enumeration('cma.ui.transport'),
  schema_version: version,
  enabled: boolean,
  selection: enumeration('new_tabs'),
  input: enumeration('responses_v2'),
  snapshot_schema: version,
  event_schema: version,
  control_schema: version,
  live_fragments: boolean,
  snapshot: enumeration('/api/v2/agents/{agent_id}/events/snapshot'),
  events: enumeration('/api/v2/agents/{agent_id}/events'),
  controls: enumeration('/api/v2/agents/{agent_id}/responses/{response_id}/controls'),
  command_status: enumeration('/api/v2/commands/{command_id}'),
});
export type V2UiTransport = Output<typeof v2UiTransport>;
const itemFields = {
  cursor,
  sequence: positiveInteger,
  appended_at: identifier,
  kind: nullable(v2AgentEventKind),
  message_command_id: optional(identifier),
  response_binding: nullable(
    strict({ command_id: identifier, response_id: identifier, turn_sequence: positiveInteger }),
  ),
};
export const v2AgentEventItem = strict(itemFields);
export type V2AgentEventItem = Output<typeof v2AgentEventItem>;
export const v2AgentEvent = strict({ ...itemFields, agent_id: identifier, incarnation });
export type V2AgentEvent = Output<typeof v2AgentEvent>;
const page = strict({
  object: enumeration('cma.agent.event_page'),
  schema_version: version,
  agent_id: identifier,
  incarnation,
  through_cursor: cursor,
  items: array(v2AgentEventItem),
  next_page_cursor: nullable(pageCursor),
});
export type V2AgentEventPage = Output<typeof page>;
export function validateItem(
  item: V2AgentEventItem,
  expectedIncarnation: string,
  sequence: number,
) {
  if (
    cursorIncarnation(item.cursor) !== expectedIncarnation ||
    cursorSequence(item.cursor) !== item.sequence ||
    item.sequence !== sequence
  )
    throw new ProtocolError('Agent event identity or chronology disagrees with its cursor');
  if (
    item.kind &&
    'turn_sequence' in item.kind &&
    item.kind.turn_sequence !== null &&
    item.kind.turn_sequence > item.sequence
  )
    throw new ProtocolError('Agent event refers to a future turn');
  if (item.kind?.type === 'turn_started' && item.kind.turn_sequence !== item.sequence)
    throw new ProtocolError('Agent turn start does not identify its own log row');
  if (
    item.message_command_id !== undefined &&
    (item.kind?.type !== 'message' || item.kind.role !== 'user')
  )
    throw new ProtocolError('Agent message command binding requires a user message');
  if (
    item.response_binding !== null &&
    (item.kind?.type !== 'turn_started' ||
      item.response_binding.turn_sequence !== item.kind.turn_sequence)
  )
    throw new ProtocolError('Agent response binding does not identify the exact turn start');
}
export function v2AgentEventPage(
  value: unknown,
  agentId: string,
  limit: number,
  previous?: V2AgentPageCursor,
): V2AgentEventPage {
  const result = page(value);
  const through = cursorSequence(result.through_cursor);
  const prior = previous === undefined ? { after: 0, through } : pageCoordinates(previous);
  if (
    result.agent_id !== agentId ||
    cursorIncarnation(result.through_cursor) !== result.incarnation ||
    (previous !== undefined && cursorIncarnation(previous) !== result.incarnation) ||
    prior.through !== through ||
    result.items.length > limit
  )
    throw new ProtocolError('Agent snapshot changed identity, watermark, or page bound');
  result.items.forEach((item, index) =>
    validateItem(item, result.incarnation, prior.after + index + 1),
  );
  const after = prior.after + result.items.length;
  if (
    after > through ||
    (result.next_page_cursor === null) !== (after === through) ||
    (after < through && result.items.length === 0)
  )
    throw new ProtocolError('Agent snapshot skipped or truncated its fixed prefix');
  if (result.next_page_cursor !== null) {
    const next = pageCoordinates(result.next_page_cursor);
    if (
      cursorIncarnation(result.next_page_cursor) !== result.incarnation ||
      next.through !== through ||
      next.after !== after
    )
      throw new ProtocolError('Agent snapshot continuation does not match its delivered page');
  }
  return result;
}
export const v2AgentLive = strict({
  agent_id: identifier,
  incarnation,
  turn_sequence: positiveInteger,
  after_sequence: integer,
  message_id: text,
  observed_at: optional(identifier),
  kind: enumeration('text_delta', 'reasoning_summary_delta'),
  delta: text,
});
export type V2AgentLive = Output<typeof v2AgentLive>;
export const v2AgentLiveGap = strict({
  agent_id: identifier,
  incarnation,
  turn_sequence: nullable(positiveInteger),
  frames: nullable(integer),
  reason: enumeration('lagged', 'reconnected'),
});
export type V2AgentLiveGap = Output<typeof v2AgentLiveGap>;
export const v2AgentStreamError = strict({ code: identifier, message: text });
export type V2AgentStreamError = Output<typeof v2AgentStreamError>;
export type V2AgentStreamFrame =
  | { readonly type: 'cma.agent.event'; readonly data: V2AgentEvent }
  | { readonly type: 'cma.agent.live'; readonly data: V2AgentLive }
  | { readonly type: 'cma.agent.live_gap'; readonly data: V2AgentLiveGap }
  | { readonly type: 'cma.agent.error'; readonly data: V2AgentStreamError };
