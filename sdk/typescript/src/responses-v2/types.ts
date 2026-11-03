import type { V2EventContext } from './event-context.js';
export type { V2EventContext } from './event-context.js';
import { v2QueuedCancellationCapability } from './queued-cancellation-types.js';
import { ProtocolError } from '../errors.js';
import {
  array,
  boolean,
  enumeration,
  integer,
  nullable,
  object,
  optional,
  tagged,
  text,
  variant,
  type Output,
} from '../schema.js';
import { conversation, outputMessage, outputText, responseFields } from '../responses/types.js';

export const v2ResponseStatus = enumeration(
  'queued',
  'in_progress',
  'completed',
  'failed',
  'incomplete',
);
export const v2ActivityItem = variant('mcp_call', {
  id: text,
  server_label: enumeration('cma'),
  name: text,
  arguments: enumeration(''),
  output: (value: unknown) => {
    if (value !== null) throw new ProtocolError('Activity output must be absent');
    return null;
  },
  'cma:output': optional(object({ text, truncated: boolean })),
  error: (value: unknown) => {
    if (value !== null) throw new ProtocolError('Activity error body must be absent');
    return null;
  },
  status: enumeration('in_progress', 'completed', 'failed'),
});
export type V2ActivityItem = Output<typeof v2ActivityItem>;
export const v2OutputItem = tagged({ message: outputMessage, mcp_call: v2ActivityItem });
export type V2OutputItem = Output<typeof v2OutputItem>;
export const v2ResponseObject = object({
  ...responseFields,
  output: array(v2OutputItem),
  status: v2ResponseStatus,
  conversation: object({ id: text }),
  completed_at: nullable(integer),
});
export type V2ResponseObject = Output<typeof v2ResponseObject>;
export type V2ResponseStatus = Output<typeof v2ResponseStatus>;
export { conversation as v2Conversation };
export type V2Conversation = Output<typeof conversation>;
const message = tagged(
  {
    user: object({
      type: enumeration('message'),
      id: text,
      status: enumeration('completed'),
      role: enumeration('user'),
      content: array(variant('input_text', { text })),
    }),
    assistant: outputMessage,
  },
  'role',
);
export const v2ConversationItemPage = object({
  object: enumeration('list'),
  data: array(message),
  first_id: nullable(text),
  last_id: nullable(text),
  has_more: boolean,
});
export type V2ConversationItemPage = Output<typeof v2ConversationItemPage>;
const sequence = { sequence_number: integer };
const output = { ...sequence, output_index: integer };
const content = { ...output, item_id: text, content_index: integer };
const lifecycle = { ...sequence, response: v2ResponseObject };
export const v2ResponseEvent = tagged({
  'response.created': variant('response.created', lifecycle),
  'response.in_progress': variant('response.in_progress', lifecycle),
  'response.completed': variant('response.completed', lifecycle),
  'response.failed': variant('response.failed', lifecycle),
  'response.incomplete': variant('response.incomplete', lifecycle),
  'response.output_item.added': variant('response.output_item.added', {
    ...output,
    item: v2OutputItem,
  }),
  'response.output_item.done': variant('response.output_item.done', {
    ...output,
    item: v2OutputItem,
  }),
  'response.mcp_call.in_progress': variant('response.mcp_call.in_progress', {
    ...output,
    item_id: text,
  }),
  'response.mcp_call.completed': variant('response.mcp_call.completed', {
    ...output,
    item_id: text,
  }),
  'response.mcp_call.failed': variant('response.mcp_call.failed', {
    ...output,
    item_id: text,
  }),
  'response.content_part.added': variant('response.content_part.added', {
    ...content,
    part: outputText,
  }),
  'response.content_part.done': variant('response.content_part.done', {
    ...content,
    part: outputText,
  }),
  'response.output_text.delta': variant('response.output_text.delta', {
    ...content,
    delta: text,
    logprobs: array((value: unknown) => value),
  }),
  'response.output_text.done': variant('response.output_text.done', {
    ...content,
    text,
    logprobs: array((value: unknown) => value),
  }),
  error: variant('error', { ...sequence, code: text, message: text, param: nullable(text) }),
});
export type V2ResponseEvent = Output<typeof v2ResponseEvent>;
export const v2Capabilities = object({
  object: enumeration('cma.responses.capabilities'),
  version: integer,
  create: text,
  agent_alias: text,
  retrieve: text,
  conversation: text,
  items: text,
  replay: object({ path: text, cursor: text, header: text, durable: boolean }),
  creation: object({
    stream: boolean,
    conversation: text,
    previous_response_id: boolean,
    store: boolean,
    input: text,
    model: text,
    idempotency_header: text,
  }),
  output: object({ standard_items: array(text), text_deltas: text, tools: text }),
  companion: text,
  documentation: text,
  controls: optional(
    object({
      path: text,
      observe: enumeration('GET'),
      submit: enumeration('POST'),
      commands: array(enumeration('cancel', 'steer', 'resolve_approval')),
      idempotency_header: enumeration('Idempotency-Key'),
      idempotency_required: (value) => {
        if (value !== true) throw new ProtocolError('response controls require idempotency');
        return true as const;
      },
      target: enumeration('bound_response_turn'),
      completion: enumeration('command_delivery'),
    }),
  ),
  command_status: optional(text),
  queued_cancellation: optional(v2QueuedCancellationCapability),
});
export type V2Capabilities = Output<typeof v2Capabilities>;
export interface V2ResponseInput {
  readonly event_context?: V2EventContext;
  readonly input:
    | string
    | readonly [
        {
          readonly role: 'user';
          readonly type?: 'message';
          readonly content:
            string | readonly { readonly type: 'input_text'; readonly text: string }[];
        },
      ];
  readonly stream: true;
  readonly metadata?: Readonly<Record<string, string>> | null;
  readonly model?: string;
  readonly store?: true;
  readonly previous_response_id?: null;
}
export interface CreateV2ResponseRequest extends V2ResponseInput {
  readonly conversation: string | { readonly id: string };
}
export interface CreateV2AgentResponseRequest extends V2ResponseInput {
  readonly conversation?: string | { readonly id: string };
}
export interface V2ResponseReceipt {
  readonly responseId: string;
  readonly commandId: string;
}
