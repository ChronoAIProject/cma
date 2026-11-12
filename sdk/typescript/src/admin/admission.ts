import { ProtocolError } from '../errors.js';
import {
  array,
  boolean,
  integer,
  nullable,
  object,
  optional,
  text,
  type Output,
} from '../schema.js';

const reason = (value: unknown): string => {
  const parsed = text(value);
  if (
    parsed.trim().length === 0 ||
    new TextEncoder().encode(parsed).length > 512 ||
    /\p{Cc}/u.test(parsed)
  )
    throw new ProtocolError('reason must contain 1 to 512 bytes without control characters');
  return parsed;
};

const operatorStop = object({ since: integer, revision: integer, actor: text, reason });
export type OperatorAdmissionStop = Output<typeof operatorStop>;

const admissionChange = object({
  revision: integer,
  at: integer,
  actor: text,
  reason,
  old_stop: nullable(operatorStop),
  new_stop: nullable(operatorStop),
});
export type AdmissionControlChange = Output<typeof admissionChange>;

const admissionFields = {
  revision: integer,
  operator_stop: nullable(operatorStop),
  last_change: nullable(admissionChange),
  writes_available: boolean,
};
export const admissionControl = object(admissionFields);
export type AdmissionControlView = Output<typeof admissionControl>;

const admissionHistory = (value: unknown): readonly AdmissionControlChange[] => {
  const changes = array(admissionChange)(value);
  if (
    changes.length > 20 ||
    changes.some(
      (change, index) => index > 0 && change.revision !== changes[index - 1]!.revision - 1,
    )
  )
    throw new ProtocolError('expected at most twenty consecutive admission changes, newest first');
  return changes;
};

const u32 = (value: unknown): number => {
  const parsed = integer(value);
  if (parsed > 0xffffffff) throw new ProtocolError('expected an unsigned 32-bit integer');
  return parsed;
};
const reserve = object({ retained_homes: nullable(u32), active_sessions: nullable(u32) });
const ceilingsShape = object({
  registrations: nullable(u32),
  retained_homes: nullable(u32),
  active_sessions: nullable(u32),
  owner_active_sessions: nullable(u32),
  existing_reserve: reserve,
});
export const admissionCeilings = (value: unknown) => {
  const parsed = ceilingsShape(value);
  for (const field of ['retained_homes', 'active_sessions'] as const) {
    if (
      parsed[field] !== null &&
      parsed.existing_reserve[field] !== null &&
      parsed.existing_reserve[field] > parsed[field]
    )
      throw new ProtocolError('existing reserve exceeds its ceiling');
  }
  return parsed;
};
export type AdmissionCeilings = Output<typeof admissionCeilings>;
const ceilingsChange = object({
  revision: integer,
  at: integer,
  actor: text,
  reason,
  old_ceilings: admissionCeilings,
  new_ceilings: admissionCeilings,
});
export type AdmissionCeilingsChange = Output<typeof ceilingsChange>;
export const admissionCeilingsState = object({
  revision: integer,
  ceilings: admissionCeilings,
  last_change: nullable(ceilingsChange),
  writes_available: boolean,
});
export type AdmissionCeilingsStateView = Output<typeof admissionCeilingsState>;
const capacityCount = object({ observed: integer, bound: integer, saturated: boolean });
const capacity = object({
  registrations: nullable(capacityCount),
  retained_homes: nullable(capacityCount),
  pending_launch_homes: optional(integer),
  active_sessions: nullable(capacityCount),
});
const ceilingsHistory = (value: unknown) => {
  const changes = array(ceilingsChange)(value);
  if (
    changes.length > 20 ||
    changes.some(
      (change, index) => index > 0 && change.revision !== changes[index - 1]!.revision - 1,
    )
  )
    throw new ProtocolError('expected at most twenty consecutive ceilings changes, newest first');
  return changes;
};
export const replaceAdmissionCeilingsRequest = object({
  ceilings: admissionCeilings,
  reason,
  expected_revision: integer,
});
export type ReplaceAdmissionCeilingsRequest = Output<typeof replaceAdmissionCeilingsRequest>;
export const admissionControlRead = object({
  ...admissionFields,
  history: admissionHistory,
  ceilings: optional(admissionCeilingsState),
  effective_limits: optional(ceilingsShape),
  capacity: optional(capacity),
  ceilings_history: optional(ceilingsHistory),
});
export type AdmissionControlReadView = Output<typeof admissionControlRead>;

export const changeAdmissionRequest = object({ reason, expected_revision: integer });
export type ChangeAdmissionRequest = Output<typeof changeAdmissionRequest>;
