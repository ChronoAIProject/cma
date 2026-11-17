import { ProtocolError } from '../errors.js';
import { integer, object, record, text, type Parser } from '../schema.js';

export const MAX_AGENT_BYTES = 4 * 1024 * 1024;
export function strict<const Fields extends Readonly<Record<string, Parser<unknown>>>>(
  fields: Fields,
) {
  const parse = object(fields);
  return (value: unknown) => {
    const source = record(value);
    if (Object.keys(source).some((key) => !Object.hasOwn(fields, key)))
      throw new ProtocolError('unsupported Agent event field');
    return parse(source);
  };
}
export function version(value: unknown): 1 {
  if (value !== 1) throw new ProtocolError('unsupported Agent event schema');
  return 1;
}
export function identifier(value: unknown): string {
  const result = text(value);
  if (
    !result.trim() ||
    new TextEncoder().encode(result).length > 256 ||
    [...result].some((char) => {
      const code = char.codePointAt(0)!;
      return code < 32 || (code >= 127 && code <= 159);
    })
  )
    throw new ProtocolError('invalid Agent event identifier');
  return result;
}
export function incarnation(value: unknown): string {
  const result = text(value);
  if (!/^[a-f0-9]{64}$/.test(result)) throw new ProtocolError('invalid Agent incarnation');
  return result;
}
function coordinate(value: string): number {
  if (!/^(0|[1-9][0-9]*)$/.test(value))
    throw new ProtocolError('noncanonical Agent cursor sequence');
  return integer(Number(value));
}
/** Opaque transport coordinate; unrelated to a standard Responses sequence. */
export type V2AgentCursor = string & { readonly __agentCursor: unique symbol };
/** Opaque fixed-head snapshot continuation token. */
export type V2AgentPageCursor = string & { readonly __agentPageCursor: unique symbol };
export function cursor(value: unknown): V2AgentCursor {
  const result = text(value);
  if (result.length > 89) throw new ProtocolError('Agent cursor exceeds its bound');
  const parts = result.split('.');
  if (parts.length !== 3 || parts[0] !== 'ag1')
    throw new ProtocolError('invalid Agent cursor family');
  incarnation(parts[1]);
  coordinate(parts[2]!);
  return result as V2AgentCursor;
}
export function pageCursor(value: unknown): V2AgentPageCursor {
  const result = text(value);
  if (result.length > 111) throw new ProtocolError('Agent page cursor exceeds its bound');
  const parts = result.split('.');
  if (parts.length !== 4 || parts[0] !== 'ags1')
    throw new ProtocolError('invalid Agent page cursor family');
  incarnation(parts[1]);
  if (coordinate(parts[3]!) >= coordinate(parts[2]!))
    throw new ProtocolError('Agent page cursor is not before its head');
  return result as V2AgentPageCursor;
}
export const cursorSequence = (value: V2AgentCursor) => coordinate(value.split('.')[2]!);
export const cursorIncarnation = (value: V2AgentCursor | V2AgentPageCursor) => value.split('.')[1]!;
export function pageCoordinates(value: V2AgentPageCursor) {
  const parts = value.split('.');
  return { through: coordinate(parts[2]!), after: coordinate(parts[3]!) };
}
