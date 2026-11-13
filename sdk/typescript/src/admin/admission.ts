import { ProtocolError } from '../errors.js';
import { array, boolean, integer, nullable, object, text, type Output } from '../schema.js';

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

export const admissionControlRead = object({ ...admissionFields, history: admissionHistory });
export type AdmissionControlReadView = Output<typeof admissionControlRead>;

export const changeAdmissionRequest = object({ reason, expected_revision: integer });
export type ChangeAdmissionRequest = Output<typeof changeAdmissionRequest>;
