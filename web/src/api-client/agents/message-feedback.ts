import type { ResolvedConfig } from '../config.js';
import type { OperationOptions } from '../http.js';
import { ProtocolError } from '../errors.js';
import {
  array,
  enumeration,
  nullable,
  object,
  positiveInteger,
  text,
  type Output,
} from '../schema.js';
import { input, query, read, segment, type EventOptions } from '../resources.js';

const messageId = (value: unknown) => {
  const parsed = text(value);
  if (!/^event:[1-9]\d*$/.test(parsed) || !Number.isSafeInteger(Number(parsed.slice(6))))
    throw new ProtocolError('expected an event:<positive sequence> message ID');
  return parsed;
};
const vote = enumeration('up', 'down');
const feedback = object({ message_id: messageId, vote: nullable(vote) });
const feedbackPage = object({
  items: array(object({ message_id: messageId, vote })),
  next: nullable(positiveInteger),
});
export type MessageVote = Output<typeof vote>;
export type MessageFeedback = Output<typeof feedback>;
export type MessageFeedbackPage = Output<typeof feedbackPage>;

export function listMessageFeedback(config: ResolvedConfig, id: string, options: EventOptions) {
  return read(
    config,
    query(`/api/v1/agents/${segment(id)}/message-feedback`, {
      after: options.after,
      limit: options.limit,
    }),
    feedbackPage,
    options,
  );
}
export function setMessageFeedback(
  config: ResolvedConfig,
  id: string,
  message: string,
  value: MessageVote | null,
  options: OperationOptions,
) {
  const expected = input(messageId, message);
  return read(
    config,
    `/api/v1/agents/${segment(id)}/messages/${expected}/feedback`,
    (raw) => {
      const parsed = feedback(raw);
      if (parsed.message_id !== expected)
        throw new ProtocolError('feedback message identity differs');
      return parsed;
    },
    {
      ...options,
      method: 'PUT',
      body: input(object({ vote: nullable(vote) }), { vote: value }),
      expectedStatus: 200,
    },
  );
}
