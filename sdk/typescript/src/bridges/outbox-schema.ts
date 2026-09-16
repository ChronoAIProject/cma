import { ProtocolError } from '../errors.js';
import {
  array,
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
import { boundedId } from './invoke-schema.js';

function closed<const Fields extends Readonly<Record<string, Parser<unknown>>>>(fields: Fields) {
  const parse = object(fields);
  return (value: unknown) => {
    if (Object.keys(record(value)).some((field) => !Object.hasOwn(fields, field)))
      throw new ProtocolError('unexpected delivery field');
    return parse(value);
  };
}
function prefixed(prefix: string) {
  return (value: unknown): string => {
    const result = text(value);
    if (!result.startsWith(prefix) || !/^[0-9a-f]{64}$/.test(result.slice(prefix.length)))
      throw new ProtocolError('invalid delivery identity');
    return result;
  };
}
export const itemId = prefixed('bro_');
export const status = enumeration(
  'pending',
  'claimed',
  'sending',
  'retry_wait',
  'sent',
  'rejected',
  'unknown',
  'abandoned',
);
const failure = enumeration(
  'credential_expired',
  'credential_revoked',
  'bot_inactive',
  'route_unavailable',
  'capability_unsupported',
  'rate_limited',
  'request_rejected',
  'connect_unavailable',
  'provider_failure',
  'response_lost',
  'worker_lost',
  'receipt_invalid',
);
const attempt = closed({
  id: prefixed('broa_'),
  number: positiveInteger,
  started_at_ms: integer,
  status,
  failure: nullable(failure),
  retained_key: boolean,
});
const itemShape = closed({
  id: itemId,
  bridge_id: boundedId,
  config_revision: positiveInteger,
  receipt_id: prefixed('brti_'),
  response_id: boundedId,
  revision: positiveInteger,
  sequence: positiveInteger,
  chunk_index: integer,
  chunk_count: positiveInteger,
  status,
  failure: nullable(failure),
  duplicate_risk: boolean,
  next_retry_at_ms: nullable(integer),
  attempts: array(attempt),
  created_at_ms: integer,
  updated_at_ms: integer,
  upstream_reconciliation_supported: boolean,
});
export type BridgeDelivery = Output<typeof itemShape>;
export type BridgeDeliveryStatus = Output<typeof status>;
export type BridgeDeliveryAttempt = Output<typeof attempt>;
export function item(value: unknown): BridgeDelivery {
  const result = itemShape(value);
  if (
    result.config_revision > 256 ||
    result.chunk_count > 1024 ||
    result.chunk_index >= result.chunk_count ||
    result.updated_at_ms < result.created_at_ms ||
    result.attempts.length > 16 ||
    result.attempts.some(
      (attempt, index) =>
        attempt.number !== index + 1 ||
        attempt.started_at_ms < result.created_at_ms ||
        attempt.started_at_ms > result.updated_at_ms,
    ) ||
    (result.status === 'retry_wait') !== (result.next_retry_at_ms !== null) ||
    (result.next_retry_at_ms !== null && result.next_retry_at_ms <= result.updated_at_ms)
  )
    throw new ProtocolError('delivery history or retry bounds are inconsistent');
  return result;
}
export const page = closed({ items: array(item), next_cursor: nullable(itemId) });
export type BridgeDeliveryPage = Output<typeof page>;
export const recoveryAction = tagged(
  {
    retry: closed({ kind: enumeration('retry'), accept_duplicate_risk: boolean }),
    abandon: closed({ kind: enumeration('abandon') }),
    reconcile: closed({ kind: enumeration('reconcile') }),
  },
  'kind',
);
export type BridgeDeliveryRecoveryAction = Output<typeof recoveryAction>;
export const recoveryRequest = closed({
  item_id: itemId,
  expected_revision: positiveInteger,
  expected_bridge_revision: positiveInteger,
  action: recoveryAction,
});
export type BridgeDeliveryRecoveryRequest = Output<typeof recoveryRequest>;
const manager = tagged(
  {
    human: closed({ kind: enumeration('human'), subject: boundedId }),
    api: closed({
      kind: enumeration('api'),
      subject: boundedId,
      delegation_id: nullable(enumeration()),
    }),
  },
  'kind',
);
const recoveryShape = closed({
  id: prefixed('brrc_'),
  bridge_id: boundedId,
  request_sha256: prefixed(''),
  actor: manager,
  action: recoveryAction,
  result: item,
  created_at_ms: integer,
});
export type BridgeDeliveryRecoveryReceipt = Output<typeof recoveryShape>;
export function recoveryReceipt(value: unknown): BridgeDeliveryRecoveryReceipt {
  const result = recoveryShape(value);
  if (
    result.bridge_id !== result.result.bridge_id ||
    result.created_at_ms < result.result.updated_at_ms
  )
    throw new ProtocolError('delivery recovery receipt changed its source or time');
  return result;
}
