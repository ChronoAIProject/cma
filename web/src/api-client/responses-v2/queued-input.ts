import type { ResolvedConfig } from '../config.js';
import { ProtocolError, ValidationError } from '../errors.js';
import { withOperationDeadline, type OperationOptions } from '../http.js';
import { input, read, query, segment, type CommandOptions } from '../resources.js';
import { controlIdentifier } from './controls-types.js';
import {
  v2QueuedInput,
  v2QueuedInputPage,
  v2QueuedInputRequest,
  type V2QueuedInputRequest,
  type V2QueuedInputPage,
  type V2QueuedInputItem,
} from './queued-input-types.js';

function path(agent: string, response: string) {
  input(controlIdentifier, agent);
  input(controlIdentifier, response);
  return `/api/v2/agents/${segment(agent)}/responses/${segment(response)}/queued-input`;
}
export async function listQueuedInputs(
  config: ResolvedConfig,
  agent: string,
  options: OperationOptions,
) {
  input(controlIdentifier, agent);
  return withOperationDeadline(config, options, async (signal): Promise<V2QueuedInputPage> => {
    const data: V2QueuedInputItem[] = [];
    const ids = new Set<string>();
    let after: number | undefined;
    for (let page = 0; page < 64; page += 1) {
      const result = await read(
        config,
        query(`/api/v2/agents/${segment(agent)}/queued-inputs`, { after }),
        v2QueuedInputPage,
        { ...options, signal },
      );
      if (
        result.agent_id !== agent ||
        result.data.some(
          (item) =>
            item.response.receipt.agent_order <= (after ?? 0) ||
            ids.has(item.response.receipt.command_id),
        )
      )
        throw new ProtocolError('queued input page identity or cursor did not match request');
      for (const item of result.data) ids.add(item.response.receipt.command_id);
      data.push(...result.data);
      if (data.length > 64)
        throw new ProtocolError('queued input traversal exceeded the admission bound');
      if (result.next_after === null) return { ...result, data };
      after = result.next_after;
    }
    throw new ProtocolError('queued input traversal did not terminate');
  });
}
export async function editQueuedInput(
  config: ResolvedConfig,
  agent: string,
  response: string,
  body: V2QueuedInputRequest,
  options: CommandOptions,
) {
  const route = path(agent, response);
  // Capture immutable intent before credential/network IO or caller mutation.
  const intent = v2QueuedInputRequest(input(v2QueuedInputRequest, body));
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
  const result = await read(config, route, v2QueuedInput, {
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
    result.target.expected_command_revision !== intent.expected_command_revision ||
    result.text !== intent.text
  )
    throw new ProtocolError('queued input receipt did not match the original source intent');
  return result;
}
