import type { ResolvedConfig } from '../config.js';
import { ProtocolError, ValidationError } from '../errors.js';
import type { OperationOptions } from '../http.js';
import { input, read, segment, type CommandOptions } from '../resources.js';
import {
  controlIdentifier,
  v2CommandStatus,
  v2ControlRequest,
  v2ResponseControl,
  v2ResponseControls,
  type V2ResponseControlRequest,
} from './controls-types.js';

function path(agent: string, response: string) {
  input(controlIdentifier, agent);
  input(controlIdentifier, response);
  return `/api/v2/agents/${segment(agent)}/responses/${segment(response)}/controls`;
}
export async function observeControls(
  config: ResolvedConfig,
  agent: string,
  response: string,
  options: OperationOptions,
) {
  const result = await read(config, path(agent, response), v2ResponseControls, options);
  if (
    result.response.receipt.agent_id !== agent ||
    result.response.receipt.response_id !== response
  )
    throw new ProtocolError('response control observation identity did not match the request');
  return result;
}
export async function submitControl(
  config: ResolvedConfig,
  agent: string,
  response: string,
  body: V2ResponseControlRequest,
  options: CommandOptions,
) {
  const route = path(agent, response);
  // Parse to a new object before credential/network IO: caller mutation must
  // not change either the sent body or the receipt comparison after awaiting.
  const intent = v2ControlRequest(input(v2ControlRequest, body));
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
  const result = await read(config, route, v2ResponseControl, {
    ...options,
    method: 'POST',
    body: intent,
    headers,
    expectedStatus: 202,
  });
  if (
    result.control.receipt.agent_id !== agent ||
    result.target.response_id !== response ||
    result.target.turn_sequence !== intent.turn_sequence ||
    result.expected_agent_revision !== intent.expected_agent_revision
  )
    throw new ProtocolError('control receipt did not match the original target and observation');
  return result;
}
export async function readCommand(config: ResolvedConfig, id: string, options: OperationOptions) {
  input(controlIdentifier, id);
  const result = await read(config, `/api/v2/commands/${segment(id)}`, v2CommandStatus, options);
  if (result.receipt.command_id !== id)
    throw new ProtocolError('command receipt identity did not match the request');
  return result;
}
