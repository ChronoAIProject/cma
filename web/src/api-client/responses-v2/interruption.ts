import type { ResolvedConfig } from '../config.js';
import { ProtocolError, ValidationError } from '../errors.js';
import { input, read, segment, type CommandOptions } from '../resources.js';
import { enumeration, positiveInteger, text, type Output } from '../schema.js';
import { controlIdentifier, strict, v2CommandStatus } from './controls-types.js';

export const v2InterruptionRequest = strict({
  turn_sequence: positiveInteger,
  expected_agent_revision: positiveInteger,
  text: (value: unknown) => {
    const parsed = text(value);
    if (!parsed.trim() || new TextEncoder().encode(parsed).length > 256 * 1024)
      throw new ProtocolError(
        'interrupt text must contain 1 through 262144 bytes of meaningful text',
      );
    return parsed;
  },
});
const interruption = strict({
  object: enumeration('cma.response.interruption'),
  control: v2CommandStatus,
  response: v2CommandStatus,
  target: strict({ response_id: controlIdentifier, turn_sequence: positiveInteger }),
  expected_agent_revision: positiveInteger,
});
/** Exact active-turn target plus new input; retain the complete body and key on retry. */
export type V2ResponseInterruptionRequest = Output<typeof v2InterruptionRequest>;
/** Atomically admitted cancellation and distinct next response; replay proves actual execution. */
export type V2ResponseInterruption = Output<typeof interruption>;

export async function interruptWithInput(
  config: ResolvedConfig,
  agent: string,
  response: string,
  body: V2ResponseInterruptionRequest,
  options: CommandOptions,
): Promise<V2ResponseInterruption> {
  input(controlIdentifier, agent);
  input(controlIdentifier, response);
  const intent = v2InterruptionRequest(input(v2InterruptionRequest, body));
  const key = options?.idempotencyKey;
  let headers: Headers;
  try {
    controlIdentifier(key);
    headers = new Headers({ 'Idempotency-Key': key });
    if (headers.get('Idempotency-Key') !== key) throw new Error('header normalization changed key');
  } catch (cause) {
    throw new ValidationError(
      'idempotencyKey must be a retained exact nonblank header of at most 256 bytes',
      { cause },
    );
  }
  const result = await read(
    config,
    `/api/v2/agents/${segment(agent)}/responses/${segment(response)}/interrupt`,
    interruption,
    { ...options, method: 'POST', body: intent, headers, expectedStatus: 202 },
  );
  const cancel = result.control;
  const next = result.response;
  if (
    result.target.response_id !== response ||
    result.target.turn_sequence !== intent.turn_sequence ||
    result.expected_agent_revision !== intent.expected_agent_revision ||
    cancel.receipt.agent_id !== agent ||
    next.receipt.agent_id !== agent ||
    cancel.receipt.response_id !== null ||
    (cancel.turn_sequence !== null && cancel.turn_sequence !== intent.turn_sequence) ||
    next.receipt.response_id === null ||
    next.receipt.response_id === response ||
    next.receipt.command_id === cancel.receipt.command_id ||
    next.receipt.agent_order !== cancel.receipt.agent_order + 1
  )
    throw new ProtocolError(
      'interruption receipt did not match the original target and new response',
    );
  return result;
}
