import { ProtocolError } from '../errors.js';
import { boolean, enumeration, integer, nullable, object, text, type Output } from '../schema.js';
import { MediaContinuationKindSchema, OutboundMessageSchema, ProblemSchema } from './types.js';

const identity = { event_id: text, conversation_id: text };
const original = object({
  ...identity,
  text: nullable(text),
  message: nullable(OutboundMessageSchema),
  error: nullable(ProblemSchema),
  response_id: text,
  media_pending: boolean,
  media_responses_url: text,
});
const response = object({
  ...identity,
  response_id: text,
  response_url: text,
  parent_response_id: text,
  delivery_id: text,
  attachment_index: integer,
  kind: MediaContinuationKindSchema,
  status: enumeration('completed', 'failed', 'incomplete'),
  text,
});
const pending = object({
  status: enumeration('pending'),
  retrieval: enumeration('channel_activity'),
});

/** Frames available only after explicit media-response negotiation. URLs are not followed. */
export type GatewayMediaControlEvent =
  | {
      readonly event: 'cmaeg.response.completed' | 'cmaeg.response.failed';
      readonly data: Output<typeof original>;
    }
  | { readonly event: 'cmaeg.media.response'; readonly data: Output<typeof response> }
  | { readonly event: 'cmaeg.media.pending'; readonly data: Output<typeof pending> };

export function mediaFrame(name: string, raw: unknown): GatewayMediaControlEvent | undefined {
  if (name === 'cmaeg.media.pending') return { event: name, data: pending(raw) };
  if (name === 'cmaeg.media.response') {
    const data = response(raw);
    if (data.attachment_index >= 16) throw new ProtocolError('invalid media attachment slot');
    return { event: name, data };
  }
  if (name === 'cmaeg.response.completed' || name === 'cmaeg.response.failed') {
    const data = original(raw);
    if (
      !data.media_pending ||
      (name === 'cmaeg.response.completed' && (data.message !== null || data.error !== null)) ||
      (name === 'cmaeg.response.failed' && data.error === null)
    )
      throw new ProtocolError('inconsistent original response outcome');
    return { event: name, data };
  }
  return undefined;
}
