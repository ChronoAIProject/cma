import { ProtocolError } from '../errors.js';
import {
  dictionary,
  enumeration,
  integer,
  object,
  optional,
  text,
  type Output,
} from '../schema.js';
import { sandboxStatus } from './types.js';

const capacity = object({
  resource: enumeration('sandbox'),
  limit: integer,
  live: integer,
  available: integer,
  counts_by_status: dictionary(integer),
  retiring_released: optional(integer),
  observed_at: text,
});

export const sandboxQuotaUsage = object({
  active: integer,
  retiring_charged: integer,
  retiring_released: integer,
});

/** Quota observation only; other admission checks and concurrent allocation still apply. */
export type SandboxCapacity = Output<typeof capacity>;

export function sandboxCapacity(value: unknown): SandboxCapacity {
  const result = capacity(value);
  let total = 0;
  for (const [name, count] of Object.entries(result.counts_by_status)) {
    if (sandboxStatus(name) === 'retired' || count === 0)
      throw new ProtocolError('Sandbox capacity contains an uncounted status');
    total += count;
  }
  if (
    !Number.isSafeInteger(total) ||
    total !== result.live ||
    result.available !== Math.max(0, result.limit - result.live) ||
    !Number.isFinite(Date.parse(result.observed_at))
  )
    throw new ProtocolError('Sandbox capacity observation is inconsistent');
  return result;
}
