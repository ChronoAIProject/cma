import type { AgentTransport } from './journal';
import { answerContext, type AnswerContext } from '@chronoai/cma-sdk';
import { parseControl, type ControlRecord } from './controlRecord';

export type InputRecord = {
  readonly answerContext?: AnswerContext;
  readonly key: string;
  readonly input: string;
} & (
  | { readonly state: 'pending'; readonly receipt: null }
  | {
      readonly state: 'accepted';
      readonly receipt: { readonly responseId: string; readonly commandId: string };
    }
);
export interface TransportRecord {
  readonly agent: string;
  readonly epoch: string;
  readonly transport: AgentTransport;
  readonly input: InputRecord | null;
  readonly control: ControlRecord | null;
}

function invalid(): never {
  throw new Error(
    'Your pending message could not be recovered. Restore browser storage before sending.',
  );
}
function object(value: unknown, fields: readonly string[]): Record<string, unknown> {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return invalid();
  const row = value as Record<string, unknown>;
  if (
    Object.keys(row).length !== fields.length ||
    fields.some((field) => !Object.hasOwn(row, field))
  )
    return invalid();
  return row;
}
function identifier(value: unknown): string {
  if (
    typeof value !== 'string' ||
    !value.trim() ||
    new TextEncoder().encode(value).length > 256 ||
    [...value].some((char) => {
      const code = char.charCodeAt(0);
      return code < 32 || (code >= 127 && code <= 159);
    })
  )
    return invalid();
  return value;
}
function inputRecord(value: unknown): InputRecord {
  const hasContext =
    typeof value === 'object' && value !== null && Object.hasOwn(value, 'answerContext');
  const row = object(value, [
    'key',
    'input',
    'state',
    'receipt',
    ...(hasContext ? ['answerContext'] : []),
  ]);
  const scope = hasContext ? { answerContext: answerContext(row.answerContext) } : {};
  const key = identifier(row.key);
  if (
    typeof row.input !== 'string' ||
    !row.input.trim() ||
    new TextEncoder().encode(row.input).length > 256 * 1024
  )
    return invalid();
  if (row.state === 'pending' && row.receipt === null)
    return { key, input: row.input, state: 'pending', receipt: null, ...scope };
  if (row.state !== 'accepted') return invalid();
  const receipt = object(row.receipt, ['responseId', 'commandId']);
  return {
    key,
    input: row.input,
    ...scope,
    state: 'accepted',
    receipt: {
      responseId: identifier(receipt.responseId),
      commandId: identifier(receipt.commandId),
    },
  };
}

export function parseRecord(source: string, agent: string, epoch: string): TransportRecord {
  let value: unknown;
  try {
    value = JSON.parse(source);
  } catch {
    return invalid();
  }
  const row = object(value, ['agent', 'epoch', 'transport', 'input', 'control']);
  if (
    row.agent !== agent ||
    row.epoch !== epoch ||
    !['agui', 'responses_v2'].includes(String(row.transport))
  )
    return invalid();
  const input = row.input === null ? null : inputRecord(row.input);
  const control = row.control === null ? null : parseControl(row.control);
  if (row.transport === 'agui' && (input !== null || control !== null)) return invalid();
  return { agent, epoch, transport: row.transport as AgentTransport, input, control };
}
