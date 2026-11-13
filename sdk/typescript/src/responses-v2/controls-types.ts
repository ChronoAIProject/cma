import { ProtocolError } from '../errors.js';
import {
  boolean,
  enumeration,
  integer,
  nullable,
  object,
  positiveInteger,
  record,
  tagged,
  text,
  type Output,
  type Parser,
} from '../schema.js';

export function strict<const Fields extends Readonly<Record<string, Parser<unknown>>>>(
  fields: Fields,
) {
  const parse = object(fields);
  return (value: unknown) => {
    const source = record(value);
    if (Object.keys(source).some((key) => !Object.hasOwn(fields, key)))
      throw new ProtocolError('unsupported response control field');
    return parse(source);
  };
}
function boundedText(value: unknown, limit: number): string {
  const parsed = text(value);
  if (
    !parsed.trim() ||
    new TextEncoder().encode(parsed).length > limit ||
    [...parsed].some((character) => {
      const code = character.codePointAt(0)!;
      return code <= 31 || (code >= 127 && code <= 159);
    })
  )
    throw new ProtocolError('invalid bounded response control identifier');
  return parsed;
}
export const controlIdentifier = (value: unknown) => boundedText(value, 256);
const steerText = (value: unknown) => {
  const parsed = text(value);
  if (!parsed.trim() || new TextEncoder().encode(parsed).length > 256 * 1024)
    throw new ProtocolError('steer text must contain 1 through 262144 bytes of meaningful text');
  return parsed;
};
export const v2ControlCommand = tagged({
  cancel: strict({ type: enumeration('cancel') }),
  steer: strict({ type: enumeration('steer'), text: steerText }),
  resolve_approval: strict({
    type: enumeration('resolve_approval'),
    approval_id: (value) => boundedText(value, 512),
    decision: enumeration('allow', 'deny', 'abort'),
  }),
});
export const v2ControlRequest = strict({
  turn_sequence: positiveInteger,
  expected_agent_revision: positiveInteger,
  command: v2ControlCommand,
});
const commandStatus = strict({
  object: enumeration('cma.agent.command'),
  receipt: strict({
    command_id: controlIdentifier,
    agent_id: controlIdentifier,
    response_id: nullable(controlIdentifier),
    admitted_at: controlIdentifier,
    agent_order: positiveInteger,
    event_cursor: integer,
  }),
  status: enumeration(
    'queued',
    'in_progress',
    'waiting_action',
    'completed',
    'failed',
    'cancelled',
  ),
  queue_deadline: controlIdentifier,
  turn_sequence: nullable(positiveInteger),
  observed_cursor: nullable(integer),
  delivery: nullable(
    enumeration(
      'dispatch_fenced',
      'write_confirmed',
      'execution_observed',
      'pre_write_failed',
      'recovery_required',
      'runtime_lost',
    ),
  ),
  failure: nullable(
    enumeration(
      'queue_timeout',
      'authority_revoked',
      'authority_unavailable',
      'credential_setup_required',
      'connections_required',
      'agent_unavailable',
      'target_turn_changed',
      'runtime_unavailable',
      'runtime_lost',
      'execution_failed',
    ),
  ),
});
export function v2CommandStatus(value: unknown): V2CommandStatus {
  const result = commandStatus(value);
  if (
    (result.delivery !== null && result.turn_sequence === null) ||
    (result.observed_cursor !== null && result.delivery === null) ||
    (result.status === 'queued' && (result.turn_sequence !== null || result.failure !== null))
  )
    throw new ProtocolError('command delivery or queued status is inconsistent');
  return result;
}
const target = strict({ response_id: controlIdentifier, turn_sequence: positiveInteger });
const observation = strict({
  object: enumeration('cma.response.controls'),
  response: v2CommandStatus,
  target: nullable(target),
  agent_revision: positiveInteger,
  target_active: boolean,
  runtime_support: strict({ cancel: boolean, steer: boolean, resolve_approval: boolean }),
});
export function v2ResponseControls(value: unknown): V2ResponseControls {
  const result = observation(value);
  if (
    result.response.receipt.response_id === null ||
    (result.target === null) !== (result.response.turn_sequence === null) ||
    (result.target !== null &&
      (result.target.response_id !== result.response.receipt.response_id ||
        result.target.turn_sequence !== result.response.turn_sequence)) ||
    (result.target_active && (result.target === null || commandIsTerminal(result.response)))
  )
    throw new ProtocolError('response control observation does not match its source command');
  return result;
}
const control = strict({
  object: enumeration('cma.response.control'),
  control: v2CommandStatus,
  target,
  expected_agent_revision: positiveInteger,
});
export function v2ResponseControl(value: unknown): V2ResponseControl {
  const result = control(value);
  if (
    result.control.receipt.response_id !== null ||
    (result.control.turn_sequence !== null &&
      result.control.turn_sequence !== result.target.turn_sequence)
  )
    throw new ProtocolError('control receipt target is inconsistent');
  return result;
}
/** Source or control command delivery state; completed control is not cancellation confirmation. */
export type V2CommandStatus = Output<typeof commandStatus>;
/** Strict immutable intent: preserve the revision, turn, command and key on retry. */
export type V2ResponseControlRequest = Output<typeof v2ControlRequest>;
export type V2ResponseControlCommand = Output<typeof v2ControlCommand>;
export type V2ResponseControls = Output<typeof observation>;
export type V2ResponseControl = Output<typeof control>;
/** Terminal command status does not establish the selected response's terminal outcome. */
export function commandIsTerminal(command: V2CommandStatus): boolean {
  return ['completed', 'failed', 'cancelled'].includes(command.status);
}
