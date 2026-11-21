import { ProtocolError } from '../errors.js';
import {
  boolean,
  enumeration,
  integer,
  object,
  optional,
  record,
  tagged,
  type Output,
} from '../schema.js';
import { reason } from './reason.js';

const signedInteger = (value: unknown): number => {
  if (typeof value !== 'number' || !Number.isSafeInteger(value))
    throw new ProtocolError('expected a signed safe integer');
  return value;
};

export const pendingHomesObservation = tagged(
  {
    disabled: object({ status: enumeration('disabled') }),
    capacity_inactive: object({ status: enumeration('capacity_inactive') }),
    index_missing: object({ status: enumeration('index_missing') }),
    index_mismatch: object({ status: enumeration('index_mismatch') }),
    unavailable: object({ status: enumeration('unavailable') }),
    verified: object({
      status: enumeration('verified'),
      expected_pending_launch_homes: integer,
      pending_launch_homes_drift: signedInteger,
    }),
  },
  'status',
);
export type PendingHomesObservation = Output<typeof pendingHomesObservation>;

const request = object({ reason, observed_pending: integer, dry_run: optional(boolean) });
export const reconcilePendingHomesRequest = (value: unknown) => {
  const source = record(value);
  if (Object.keys(source).some((key) => !['reason', 'observed_pending', 'dry_run'].includes(key)))
    throw new ProtocolError('unexpected pending home correction field');
  return request(source);
};
export type ReconcilePendingHomesRequest = Output<typeof reconcilePendingHomesRequest>;

export const pendingHomesReconciliation = object({
  previous_pending_launch_homes: integer,
  expected_pending_launch_homes: integer,
  pending_launch_homes: integer,
  dry_run: boolean,
  corrected: boolean,
});
export type PendingHomesReconciliation = Output<typeof pendingHomesReconciliation>;
