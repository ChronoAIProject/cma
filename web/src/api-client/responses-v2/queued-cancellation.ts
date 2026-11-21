import type { ResolvedConfig } from '../config.js';
import { ProtocolError, ValidationError } from '../errors.js';
import type { OperationOptions } from '../http.js';
import { input, read, segment, type CommandOptions } from '../resources.js';
import { controlIdentifier } from './controls-types.js';
import {
  v2QueuedCancellation,
  v2QueuedCancellationObservation,
  v2QueuedCancellationRequest,
  type V2QueuedCancellationRequest,
} from './queued-cancellation-types.js';

function path(agent: string, response: string) {
  input(controlIdentifier, agent);
  input(controlIdentifier, response);
  return `/api/v2/agents/${segment(agent)}/responses/${segment(response)}/queued-cancellation`;
}
export async function observeQueuedCancellation(
  config: ResolvedConfig,
  agent: string,
  response: string,
  options: OperationOptions,
) {
  const result = await read(
    config,
    path(agent, response),
    v2QueuedCancellationObservation,
    options,
  );
  if (
    result.response.receipt.agent_id !== agent ||
    result.response.receipt.response_id !== response
  )
    throw new ProtocolError('queued cancellation observation identity did not match the request');
  return result;
}
export async function cancelQueuedResponse(
  config: ResolvedConfig,
  agent: string,
  response: string,
  body: V2QueuedCancellationRequest,
  options: CommandOptions,
) {
  const route = path(agent, response);
  // Capture immutable intent before credential/network IO or caller mutation.
  const intent = v2QueuedCancellationRequest(input(v2QueuedCancellationRequest, body));
  const key = options?.idempotencyKey;
  let headers: Headers;
  try {
    controlIdentifier(key);
    headers = new Headers({ 'Idempotency-Key': key });
    if (headers.get('Idempotency-Key') !== key) throw new Error('header normalization changed key');
  } catch (cause) {
    throw new ValidationError(
      'idempotencyKey must be a retained nonblank header of at most 256 bytes, preserved exactly',
      { cause },
    );
  }
  const result = await read(config, route, v2QueuedCancellation, {
    ...options,
    method: 'POST',
    body: intent,
    headers,
    expectedStatus: 200,
  });
  if (
    result.response.receipt.agent_id !== agent ||
    result.target.response_id !== response ||
    result.target.source_command_id !== intent.source_command_id ||
    result.target.expected_command_revision !== intent.expected_command_revision
  )
    throw new ProtocolError('queued cancellation receipt did not match the original source intent');
  return result;
}
