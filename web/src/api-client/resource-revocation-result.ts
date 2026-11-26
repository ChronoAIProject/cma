import { ProtocolError } from './errors.js';
import { enumeration, object, positiveInteger, record, text } from './schema.js';

export interface RevocationResult {
  readonly revocation?: { readonly operation: string };
}

const reference = object({
  operation_id: text,
  kind: enumeration('agent', 'sandbox'),
  id: text,
  share_id: text,
  expected_revision: positiveInteger,
});

/** Present metadata requires observing the original operation before discarding recovery. */
export function revocationReference(
  raw: unknown,
  kind: string,
  id: string,
  shareId: string,
  revision: number,
): { readonly operation: string } | undefined {
  const source = record(raw);
  if (source.revocation === undefined) return undefined;
  const value = reference(source.revocation);
  if (
    !/^[a-f0-9]{64}$/.test(value.operation_id) ||
    value.kind !== kind ||
    value.id !== id ||
    value.share_id !== shareId ||
    value.expected_revision !== revision
  )
    throw new ProtocolError('Revocation result does not match the original request');
  return { operation: value.operation_id };
}
