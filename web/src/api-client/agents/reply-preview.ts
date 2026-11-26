import type { ResolvedConfig } from '../config.js';
import type { OperationOptions } from '../http.js';
import { ProtocolError } from '../errors.js';
import {
  boolean,
  enumeration,
  integer,
  nullable,
  object,
  positiveInteger,
  tagged,
  text,
  type Output,
} from '../schema.js';
import { read, segment } from '../resources.js';

const absent = (value: unknown): null => {
  if (value !== null) throw new ProtocolError('expected no assistant reply');
  return null;
};
const reply = object({
  text,
  truncated: boolean,
  message_sequence: positiveInteger,
  turn_sequence: positiveInteger,
  appended_at: text,
});
const preview = tagged(
  {
    ready: object({
      status: enumeration('ready'),
      agent_id: text,
      source_through_seq: integer,
      latest_assistant_reply: reply,
    }),
    none: object({
      status: enumeration('none'),
      agent_id: text,
      source_through_seq: integer,
      latest_assistant_reply: absent,
    }),
    unavailable: object({
      status: enumeration('unavailable'),
      agent_id: text,
      source_through_seq: nullable(integer),
      latest_assistant_reply: absent,
    }),
  },
  'status',
);

export type AgentReplyPreview = Output<typeof preview>;

function parsePreview(value: unknown): AgentReplyPreview {
  const result = preview(value);
  if (result.status !== 'ready') return result;
  const message = result.latest_assistant_reply;
  if (
    !message.text.trim() ||
    message.text !== message.text.trim() ||
    [...message.text].length > 240 ||
    /[\p{Cc}\p{Cs}]/u.test(message.text) ||
    /\s{2,}/u.test(message.text) ||
    !Number.isFinite(Date.parse(message.appended_at)) ||
    message.turn_sequence >= message.message_sequence ||
    message.message_sequence > result.source_through_seq
  )
    throw new ProtocolError('Agent reply preview has invalid source evidence or text');
  return result;
}

export function readReplyPreview(
  config: ResolvedConfig,
  id: string,
  options: OperationOptions = {},
) {
  return read(
    config,
    `/api/v1/agents/${segment(id)}/reply-preview`,
    (value) => {
      const result = parsePreview(value);
      if (result.agent_id !== id)
        throw new ProtocolError('reply preview Agent identity did not match the request');
      return result;
    },
    { ...options, cache: 'no-store' },
  );
}
