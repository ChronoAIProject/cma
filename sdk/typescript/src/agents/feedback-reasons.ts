import type { ResolvedConfig } from '../config.js';
import type { OperationOptions } from '../http.js';
import { ProtocolError } from '../errors.js';
import { array, enumeration, integer, nullable, object, text, type Output } from '../schema.js';
import { input, read, segment } from '../resources.js';

const reason = enumeration(
  'inaccurate',
  'unanswered',
  'unusable_steps',
  'incomplete',
  'unclear',
  'other',
);
const reasons = (value: unknown) => {
  const parsed = array(reason)(value);
  if (new Set(parsed).size !== parsed.length) throw new ProtocolError('reasons must be unique');
  return parsed;
};
const comment = (value: unknown) => {
  const parsed = text(value);
  if (Array.from(parsed).length > 500)
    throw new ProtocolError('comment exceeds 500 Unicode code points');
  return parsed;
};
const requestId = (value: unknown) => {
  const parsed = text(value);
  if (!/^[A-Za-z0-9_.:-]{1,128}$/.test(parsed))
    throw new ProtocolError('invalid feedback request ID');
  return parsed;
};
const request = object({ expected_version: integer, request_id: requestId, reasons, comment });
const details = object({
  viewer_id: text,
  agent_id: text,
  message_id: text,
  vote: nullable(enumeration('up', 'down')),
  version: integer,
  reasons,
  comment,
  updated_at: nullable(text),
  request_id: nullable(requestId),
});
export type FeedbackReason = Output<typeof reason>;
export type FeedbackReasonRequest = Output<typeof request>;
export type FeedbackDetails = Output<typeof details>;

function path(agent: string, message: string) {
  const identity = input((value: unknown) => {
    const parsed = text(value);
    if (!/^event:[1-9]\d*$/.test(parsed) || !Number.isSafeInteger(Number(parsed.slice(6))))
      throw new ProtocolError('expected a durable message ID');
    return parsed;
  }, message);
  return `/api/v1/agents/${segment(agent)}/messages/${identity}/feedback`;
}
function receipt(raw: unknown, agent: string, message: string) {
  const parsed = details(raw);
  if (
    parsed.agent_id !== agent ||
    parsed.message_id !== message ||
    (parsed.vote !== 'down' && (parsed.reasons.length !== 0 || parsed.comment !== ''))
  )
    throw new ProtocolError('feedback identity or vote differs');
  return parsed;
}
export function feedbackDetails(
  config: ResolvedConfig,
  agent: string,
  message: string,
  options: OperationOptions,
) {
  return read(config, path(agent, message), (raw) => receipt(raw, agent, message), options);
}
export function setFeedbackReasons(
  config: ResolvedConfig,
  agent: string,
  message: string,
  body: FeedbackReasonRequest,
  options: OperationOptions,
) {
  const expected = input(request, body);
  return read(
    config,
    `${path(agent, message)}/reasons`,
    (raw) => {
      const parsed = receipt(raw, agent, message);
      if (
        parsed.vote !== 'down' ||
        parsed.version !== expected.expected_version + 1 ||
        parsed.request_id !== expected.request_id ||
        parsed.comment !== expected.comment ||
        JSON.stringify(parsed.reasons) !== JSON.stringify(expected.reasons)
      )
        throw new ProtocolError('feedback reason receipt differs from the request');
      return parsed;
    },
    { ...options, method: 'PUT', body: expected, expectedStatus: 200 },
  );
}
