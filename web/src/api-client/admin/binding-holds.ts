import { ProtocolError } from '../errors.js';
import {
  boolean,
  enumeration,
  integer,
  nullable,
  object,
  optional,
  text,
  type Output,
  type Parser,
} from '../schema.js';

const boundedText =
  (limit: number): Parser<string> =>
  (value) => {
    const result = text(value);
    if (
      !result.trim() ||
      result.length > limit ||
      [...result].some(
        (character) => character.charCodeAt(0) < 32 || character.charCodeAt(0) === 127,
      )
    )
      throw new ProtocolError('invalid binding observation text');
    return result;
  };

const timestamp: Parser<string> = (value) => {
  const result = boundedText(64)(value);
  if (
    !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$/u.test(result) ||
    !Number.isFinite(Date.parse(result))
  )
    throw new ProtocolError('invalid binding observation time');
  return result;
};

const boundedCount =
  (limit: number): Parser<number> =>
  (value) => {
    const result = integer(value);
    if (result > limit) throw new ProtocolError('binding observation count exceeds its bound');
    return result;
  };

const boundedList =
  <Value>(parse: Parser<Value>, limit: number): Parser<readonly Value[]> =>
  (value) => {
    if (!Array.isArray(value) || value.length > limit)
      throw new ProtocolError('binding observation list exceeds its bound');
    return value.map((entry: unknown) => parse(entry));
  };

const hold = object({
  sandbox_id: boundedText(512),
  owner: boundedText(512),
  owner_name: optional(nullable(boundedText(512))),
  reason: enumeration(
    'unavailable',
    'raced',
    'cancelled',
    'deadline',
    'corrupt',
    'conflict',
    'refused',
  ),
  phase: enumeration('deferred', 'retrying'),
  first_held_at: timestamp,
  attempts: boundedCount(4_294_967_295),
  retry_not_before: nullable(timestamp),
  adapter_error_class: nullable(enumeration('unavailable')),
});
export type BindingHold = Output<typeof hold>;

const report = object({
  reporter: boundedText(128),
  sample: object({ observed_at: timestamp, truncated: boolean, holds: boundedList(hold, 64) }),
});
const overview = object({
  observed_at: timestamp,
  observed_sandboxes: boundedCount(1024),
  registered_reporters: boundedCount(16),
  unavailable_reporters: boundedCount(16),
  truncated: boolean,
  reports: boundedList(report, 16),
});
export type BindingHoldOverview = Output<typeof overview>;

export const bindingHoldOverview: Parser<BindingHoldOverview> = (value) => {
  const parsed = overview(value);
  const reporters = new Set<string>();
  const sandboxes = new Set<string>();
  for (const observation of parsed.reports) {
    if (reporters.has(observation.reporter))
      throw new ProtocolError('duplicate binding observation reporter');
    reporters.add(observation.reporter);
    const local = new Set<string>();
    for (const row of observation.sample.holds) {
      if (row.attempts === 0 || local.has(row.sandbox_id))
        throw new ProtocolError('invalid or duplicate held binding');
      if ((row.phase === 'retrying') !== (row.retry_not_before === null))
        throw new ProtocolError('binding retry phase does not match its observation');
      local.add(row.sandbox_id);
      sandboxes.add(row.sandbox_id);
    }
  }
  if (
    parsed.observed_sandboxes !== sandboxes.size ||
    parsed.registered_reporters !== parsed.reports.length + parsed.unavailable_reporters ||
    (!parsed.truncated && parsed.reports.some((entry) => entry.sample.truncated))
  )
    throw new ProtocolError('inconsistent binding observation summary');
  return parsed;
};
