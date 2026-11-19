import { ProtocolError } from '../errors.js';
import {
  array,
  boolean,
  enumeration,
  integer,
  object,
  optional,
  record,
  text,
  type Output,
} from '../schema.js';

const spendLevels = object({ warning: integer, stop: integer, disposable: integer });
export type SpendLevels = Output<typeof spendLevels>;

const cloudObservation = object({
  observed_through: text,
  units: integer,
  recorded_at: text,
  settings_revision: integer,
});
export type CloudObservation = Output<typeof cloudObservation>;

const spendClosure = object({
  reason: enumeration('spend_warning', 'spend_stop', 'metering_unavailable'),
  since: text,
  settings_revision: integer,
  committed_at_entry: integer,
});
export type SpendClosure = Output<typeof spendClosure>;

const spendLedger = object({
  period_start: text,
  period_end: text,
  settings_revision: integer,
  spent: integer,
  reserved: integer,
  adjustments: integer,
  cloud: optional(cloudObservation),
  closures: array(spendClosure),
  evaluated_at: optional(text),
});
export type SpendLedger = Output<typeof spendLedger>;

/** Disabled metering is distinct from an enabled period or an unavailable response. */
export const platformSpend = object({
  metering_enabled: boolean,
  settings_revision: integer,
  levels: optional(spendLevels),
  committed: optional(integer),
  ledger: optional(spendLedger),
});
export type PlatformSpend = Output<typeof platformSpend>;

const observationFields = {
  source: enumeration('cloud'),
  observed_through: text,
  units: integer,
};
const observation = object(observationFields);
export type SpendObservationRequest = Output<typeof observation>;

/** The server validates RFC 3339, its current clock, period bounds and conflicting retries. */
export function spendObservationRequest(value: unknown): SpendObservationRequest {
  if (Object.keys(record(value)).some((key) => !Object.hasOwn(observationFields, key)))
    throw new ProtocolError('unknown platform spend observation field');
  return observation(value);
}
