import { ProtocolError } from '../errors.js';
import {
  enumeration,
  integer,
  nullable,
  positiveInteger,
  text,
  object,
  record,
  type Parser,
  type Output,
} from '../schema.js';
import { actor, boundedId } from './invoke-schema.js';
import { allocationId } from './cleanup-schema.js';

function closed<const Fields extends Readonly<Record<string, Parser<unknown>>>>(fields: Fields) {
  const parse = object(fields);
  return (value: unknown) => {
    if (Object.keys(record(value)).some((field) => !Object.hasOwn(fields, field)))
      throw new ProtocolError('unexpected partition command field');
    return parse(value);
  };
}
const action = enumeration('reset', 'detach');
export type PartitionCommandAction = Output<typeof action>;
export const request = closed({
  expected_bridge_revision: positiveInteger,
  expected_revision: positiveInteger,
});
export type PartitionCommandRequest = Output<typeof request>;
function generation(value: unknown) {
  const result = positiveInteger(value);
  if (result > 256) throw new ProtocolError('partition generation exceeds 256');
  return result;
}
function digest(value: unknown) {
  const result = text(value);
  if (!/^[0-9a-f]{64}$/.test(result)) throw new ProtocolError('invalid partition command digest');
  return result;
}
function receiptId(value: unknown) {
  const result = text(value);
  if (!/^bpc_[0-9a-f]{64}$/.test(result))
    throw new ProtocolError('invalid partition command receipt identity');
  return result;
}
const result = closed({
  id: allocationId,
  generation,
  revision: positiveInteger,
  state: enumeration('retired', 'reserved', 'ready'),
});
export type PartitionCommandResult = Output<typeof result>;
const receiptValue = closed({
  id: receiptId,
  bridge_id: boundedId,
  request_sha256: digest,
  actor,
  action,
  expected_bridge_revision: positiveInteger,
  allocation_id: allocationId,
  generation,
  previous_revision: positiveInteger,
  result,
  successor: nullable(result),
  created_at_ms: integer,
});
export function receipt(value: unknown) {
  const parsed = receiptValue(value);
  if (
    parsed.result.id !== parsed.allocation_id ||
    parsed.result.generation !== parsed.generation ||
    parsed.result.revision !== parsed.previous_revision + 1 ||
    parsed.result.state !== 'retired'
  )
    throw new ProtocolError('partition command changed its original generation');
  const successor = parsed.successor;
  if (
    parsed.action === 'detach'
      ? successor !== null
      : successor === null ||
        successor.id === parsed.allocation_id ||
        successor.generation !== parsed.generation + 1 ||
        successor.revision !== 1 ||
        !['reserved', 'ready'].includes(successor.state)
  )
    throw new ProtocolError('partition command has an inconsistent successor');
  return parsed;
}
export type PartitionCommandReceipt = Output<typeof receipt>;
