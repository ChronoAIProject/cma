import {
  array,
  boolean,
  dictionary,
  enumeration,
  integer,
  nullable,
  object,
  optional,
  record,
  tagged,
  text,
  variant,
  type Output,
} from '../schema.js';
import { ProtocolError } from '../errors.js';

const finiteNumber = (value: unknown) => {
  if (typeof value !== 'number' || !Number.isFinite(value))
    throw new ProtocolError('expected a finite number');
  return value;
};
const jsonValue = (value: unknown) => value;
export const responseStatus = enumeration('in_progress', 'completed', 'failed', 'incomplete');
export type ResponseStatus = Output<typeof responseStatus>;
export const outputText = variant('output_text', {
  text,
  annotations: array(jsonValue),
  logprobs: array(jsonValue),
});
export const outputMessage = variant('message', {
  id: text,
  status: enumeration('in_progress', 'completed'),
  role: enumeration('assistant'),
  content: array(outputText),
});
export type ResponseOutputMessage = Output<typeof outputMessage>;
export const summaryText = variant('summary_text', { text });
export const reasoningItem = variant('reasoning', {
  id: text,
  status: enumeration('in_progress', 'completed'),
  summary: array(summaryText),
  'cma:live': boolean,
});
export const responseItem = tagged({ message: outputMessage, reasoning: reasoningItem });
export const toolActivity = variant('cma_chat.tool_activity', {
  id: text,
  call_id: text,
  kind: text,
  target: nullable(text),
  status: text,
  duration_ms: nullable(integer),
});
export type ResponseToolActivity = Output<typeof toolActivity>;
export const conversation = object({
  id: text,
  object: enumeration('conversation'),
  created_at: integer,
  metadata: dictionary(text),
});
export type Conversation = Output<typeof conversation>;
export const responseFields = {
  callback_url: optional(nullable(text)),
  id: text,
  object: enumeration('response'),
  created_at: integer,
  status: responseStatus,
  background: boolean,
  error: nullable(object({ code: text, message: text })),
  incomplete_details: nullable(object({ reason: enumeration('interrupted') })),
  instructions: nullable(text),
  max_output_tokens: nullable(integer),
  model: text,
  output: array(outputMessage),
  parallel_tool_calls: boolean,
  previous_response_id: nullable(text),
  reasoning: object({ effort: nullable(text), summary: nullable(text) }),
  store: boolean,
  temperature: nullable(finiteNumber),
  text: object({ format: object({ type: enumeration('text') }), verbosity: enumeration('medium') }),
  tool_choice: enumeration('none'),
  tools: array(record),
  top_p: nullable(finiteNumber),
  truncation: enumeration('disabled'),
  usage: nullable(
    object({
      input_tokens: integer,
      input_tokens_details: object({ cached_tokens: integer }),
      output_tokens: integer,
      output_tokens_details: object({ reasoning_tokens: integer }),
      total_tokens: integer,
    }),
  ),
  metadata: dictionary(text),
};
export const responseObject = object(responseFields);
export type ResponseObject = Output<typeof responseObject>;

const conversationMessage = tagged(
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
const conversationItem = tagged({
  message: conversationMessage,
  'cma_chat.tool_activity': toolActivity,
});
export type ConversationItem = Output<typeof conversationItem>;
export const conversationItemPage = object({
  object: enumeration('list'),
  data: array(conversationItem),
  first_id: nullable(text),
  last_id: nullable(text),
  has_more: boolean,
});
export type ConversationItemPage = Output<typeof conversationItemPage>;

export const createConversationRequest = object({
  metadata: optional(
    nullable(
      object({
        agent_profile: optional(text),
        workspace: optional(text),
        sandbox: optional(text),
      }),
    ),
  ),
});
export type CreateConversationRequest = Output<typeof createConversationRequest>;

export interface CreateResponseRequest {
  readonly conversation: string;
  readonly input:
    | string
    | readonly [
        {
          readonly role: 'user';
          readonly type?: 'message';
          readonly content:
            string | readonly [{ readonly type: 'input_text'; readonly text: string }];
        },
      ];
  readonly stream: true;
  readonly metadata?: Readonly<Record<string, string>> | null;
  readonly previous_response_id?: string | null;
  /** Accepted for compatibility; the response reports the model selected for its Agent turn. */
  readonly model?: string;
  readonly store?: true;
}
