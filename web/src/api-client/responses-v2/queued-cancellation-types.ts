import { ProtocolError } from '../errors.js';
import { boolean, enumeration, positiveInteger, type Output } from '../schema.js';
import { controlIdentifier, strict, v2CommandStatus } from './controls-types.js';

export const v2QueuedCancellationRequest = strict({
  source_command_id: controlIdentifier,
  expected_command_revision: positiveInteger,
});
const observation = strict({
  object: enumeration('cma.response.queued_cancellation.observation'),
  response: v2CommandStatus,
  command_revision: positiveInteger,
  cancellable: boolean,
});
export function v2QueuedCancellationObservation(value: unknown): V2QueuedCancellationObservation {
  const result = observation(value);
  if (
    result.response.receipt.response_id === null ||
    (result.cancellable &&
      (!['queued', 'waiting_action'].includes(result.response.status) ||
        result.response.turn_sequence !== null ||
        result.response.delivery !== null ||
        result.response.observed_cursor !== null ||
        result.response.failure !== null))
  )
    throw new ProtocolError('queued cancellation observation is inconsistent with its source');
  return result;
}
const target = strict({
  response_id: controlIdentifier,
  source_command_id: controlIdentifier,
  expected_command_revision: positiveInteger,
});
const cancellation = strict({
  object: enumeration('cma.response.queued_cancellation'),
  cancellation: v2CommandStatus,
  response: v2CommandStatus,
  target,
});
export function v2QueuedCancellation(value: unknown): V2QueuedCancellation {
  const result = cancellation(value);
  const receipt = result.cancellation;
  if (
    receipt.receipt.response_id !== null ||
    receipt.status !== 'completed' ||
    receipt.turn_sequence !== null ||
    receipt.delivery !== null ||
    receipt.observed_cursor !== null ||
    receipt.failure !== null ||
    receipt.receipt.command_id === result.target.source_command_id ||
    receipt.receipt.agent_id !== result.response.receipt.agent_id ||
    result.response.receipt.response_id !== result.target.response_id ||
    result.response.receipt.command_id !== result.target.source_command_id
  )
    throw new ProtocolError('queued cancellation receipt or source identity is inconsistent');
  return result;
}
export const v2QueuedCancellationCapability = strict({
  path: controlIdentifier,
  observe: enumeration('GET'),
  submit: enumeration('POST'),
  idempotency_header: enumeration('Idempotency-Key'),
  idempotency_required: (value) => {
    if (value !== true) throw new ProtocolError('queued cancellation requires idempotency');
    return true as const;
  },
  target: enumeration('unbound_response_command'),
  completion: enumeration('source_cancelled'),
});
/** Persist this exact source command, revision, route and key before sending. */
export type V2QueuedCancellationRequest = Output<typeof v2QueuedCancellationRequest>;
/** A current eligibility observation never substitutes for fresh operation authority. */
export type V2QueuedCancellationObservation = Output<typeof observation>;
/** Committed cancellation receipt and the source command's current safe status. */
export type V2QueuedCancellation = Output<typeof cancellation>;
