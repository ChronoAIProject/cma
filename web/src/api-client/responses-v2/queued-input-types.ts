import { ProtocolError } from '../errors.js';
import {
  array,
  boolean,
  enumeration,
  positiveInteger,
  nullable,
  text,
  type Output,
} from '../schema.js';
import { controlIdentifier, strict, v2CommandStatus } from './controls-types.js';
function inputText(value: unknown): string {
  const result = text(value);
  if (!result.trim() || new TextEncoder().encode(result).length > 256 * 1024)
    throw new ProtocolError('queued input text must contain 1 to 262144 bytes');
  return result;
}
export const v2QueuedInputRequest = strict({
  source_command_id: controlIdentifier,
  expected_command_revision: positiveInteger,
  text: inputText,
});
const item = strict({
  response: v2CommandStatus,
  command_revision: positiveInteger,
  text: inputText,
  editable: boolean,
});
export function v2QueuedInputItem(value: unknown) {
  const result = item(value);
  if (
    result.response.receipt.response_id === null ||
    result.response.turn_sequence !== null ||
    result.response.delivery !== null ||
    result.response.failure !== null ||
    !['queued', 'waiting_action'].includes(result.response.status)
  )
    throw new ProtocolError('queued input must identify a response before dispatch');
  return result;
}
const page = strict({
  object: enumeration('cma.agent.queued_inputs'),
  agent_id: controlIdentifier,
  data: array(v2QueuedInputItem),
  next_after: nullable(positiveInteger),
});
export function v2QueuedInputPage(value: unknown) {
  const result = page(value);
  if (
    result.data.length > 4 ||
    (result.next_after !== null &&
      result.next_after !== result.data.at(-1)?.response.receipt.agent_order) ||
    new Set(result.data.map((entry) => entry.response.receipt.command_id)).size !==
      result.data.length ||
    result.data.some(
      (entry, index) =>
        entry.response.receipt.agent_id !== result.agent_id ||
        (index > 0 &&
          entry.response.receipt.agent_order <=
            result.data[index - 1]!.response.receipt.agent_order),
    )
  )
    throw new ProtocolError('queued input page identity or order is invalid');
  return result;
}
const target = strict({
  response_id: controlIdentifier,
  source_command_id: controlIdentifier,
  expected_command_revision: positiveInteger,
});
const edit = strict({
  object: enumeration('cma.response.queued_input'),
  edit: v2CommandStatus,
  response: v2CommandStatus,
  target,
  command_revision: positiveInteger,
  text: inputText,
});
export function v2QueuedInput(value: unknown) {
  const result = edit(value);
  if (
    result.edit.receipt.response_id !== null ||
    result.edit.status !== 'completed' ||
    result.edit.turn_sequence !== null ||
    result.edit.delivery !== null ||
    result.edit.failure !== null ||
    result.edit.receipt.command_id === result.target.source_command_id ||
    result.edit.receipt.agent_id !== result.response.receipt.agent_id ||
    result.response.receipt.command_id !== result.target.source_command_id ||
    result.response.receipt.response_id !== result.target.response_id ||
    result.command_revision !== result.target.expected_command_revision + 1
  )
    throw new ProtocolError('queued input edit receipt or revision is inconsistent');
  return result;
}
export type V2QueuedInputRequest = Output<typeof v2QueuedInputRequest>;
export type V2QueuedInputItem = Output<typeof v2QueuedInputItem>;
export type V2QueuedInputPage = Output<typeof v2QueuedInputPage>;
export type V2QueuedInput = Output<typeof v2QueuedInput>;
