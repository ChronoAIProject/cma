import {
  array,
  boolean,
  integer,
  optional,
  tagged,
  text,
  variant,
  type Output,
} from '../schema.js';
import { outputText, responseItem, responseObject, summaryText, toolActivity } from './types.js';

const sequence = { sequence_number: integer };
const output = { ...sequence, output_index: integer };
const content = { ...output, item_id: text, content_index: integer };
const logprobs = optional(array((value: unknown) => value));
const summary = { ...output, item_id: text, summary_index: integer };
const lifecycle = { ...sequence, response: responseObject };

export const responseEvent = tagged({
  'response.created': variant('response.created', lifecycle),
  'response.in_progress': variant('response.in_progress', lifecycle),
  'response.completed': variant('response.completed', lifecycle),
  'response.failed': variant('response.failed', lifecycle),
  'response.incomplete': variant('response.incomplete', lifecycle),
  'response.output_item.added': variant('response.output_item.added', {
    ...output,
    item: responseItem,
  }),
  'response.output_item.done': variant('response.output_item.done', {
    ...output,
    item: responseItem,
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
    logprobs,
  }),
  'response.output_text.done': variant('response.output_text.done', { ...content, text, logprobs }),
  'response.reasoning_summary_part.added': variant('response.reasoning_summary_part.added', {
    ...summary,
    part: summaryText,
  }),
  'response.reasoning_summary_part.done': variant('response.reasoning_summary_part.done', {
    ...summary,
    part: summaryText,
  }),
  'response.reasoning_summary_text.delta': variant('response.reasoning_summary_text.delta', {
    ...summary,
    delta: text,
  }),
  'response.reasoning_summary_text.done': variant('response.reasoning_summary_text.done', {
    ...summary,
    text,
    'cma:truncated': optional(boolean),
  }),
  'response.cma_chat.tool_activity': variant('response.cma_chat.tool_activity', {
    ...sequence,
    response_id: text,
    item: toolActivity,
  }),
});
export type ResponseEvent = Output<typeof responseEvent>;
