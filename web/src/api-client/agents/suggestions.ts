import type { ResolvedConfig } from '../config.js';
import type { OperationOptions } from '../http.js';
import type { AnswerContext } from '../answer-context.js';
import { ProtocolError, ValidationError } from '../errors.js';
import {
  array,
  enumeration,
  integer,
  nullable,
  object,
  optional,
  positiveInteger,
  text,
  type Output,
} from '../schema.js';
import { query, read, segment } from '../resources.js';

const scope = object({ turn_sequence: positiveInteger, answer_sequence: positiveInteger });
const source = object({ kind: enumeration('template', 'generated'), version: text });
const set = object({
  id: text,
  viewerId: text,
  agentId: text,
  answerId: text,
  scope,
  source,
  items: array(object({ id: text, text, rank: integer })),
});
const usage = object({
  model: nullable(text),
  input_tokens: nullable(integer),
  cached_input_tokens: nullable(integer),
  output_tokens: nullable(integer),
  cost: nullable(object({ amount: text, currency: text })),
});
const reading = object({
  status: enumeration('pending', 'running', 'ready', 'empty', 'failed', 'expired', 'unavailable'),
  set: nullable(set),
  observation: optional(
    nullable(
      object({
        elapsed_ms: integer,
        usage,
        failure: nullable(enumeration('unavailable', 'timeout', 'invalid_response', 'interrupted')),
      }),
    ),
  ),
});
export type SuggestedReplySet = Output<typeof set>;
export type SuggestedReplyReading = Output<typeof reading>;

export async function readSuggestions(
  config: ResolvedConfig,
  agentId: string,
  answer: AnswerContext,
  options: OperationOptions,
) {
  if (
    !Number.isSafeInteger(answer.turn_sequence) ||
    !Number.isSafeInteger(answer.answer_sequence) ||
    answer.turn_sequence < 1 ||
    answer.answer_sequence <= answer.turn_sequence
  )
    throw new ValidationError('suggestions require a canonical completed answer scope');
  const result = await read(
    config,
    query(`/api/v1/agents/${segment(agentId)}/suggestions`, { ...answer }),
    reading,
    options,
  );
  const saved = result.set;
  if (
    (result.status === 'ready') !== (saved !== null) ||
    (saved &&
      (saved.agentId !== agentId ||
        saved.answerId !== `event:${answer.answer_sequence}` ||
        saved.scope.turn_sequence !== answer.turn_sequence ||
        saved.scope.answer_sequence !== answer.answer_sequence ||
        !saved.id.trim() ||
        !saved.viewerId.trim() ||
        !saved.source.version.trim() ||
        saved.items.length < 1 ||
        saved.items.length > 3 ||
        saved.items.some(
          (item, index) =>
            !item.id.trim() ||
            !item.text.trim() ||
            new TextEncoder().encode(item.text).length > 256 * 1024 ||
            (index > 0 && item.rank <= saved.items[index - 1]!.rank),
        ) ||
        new Set(saved.items.map((item) => item.id)).size !== saved.items.length ||
        new Set(saved.items.map((item) => item.text.trim().toLowerCase())).size !==
          saved.items.length))
  )
    throw new ProtocolError('Agent Manager returned invalid or mismatched suggestions');
  return result;
}
