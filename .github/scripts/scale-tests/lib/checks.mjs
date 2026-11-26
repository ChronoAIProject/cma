import { createHash } from 'node:crypto';
import { setTimeout } from 'node:timers/promises';

export class GateError extends Error {
  constructor(code, blocked = false) {
    super(code);
    this.code = code;
    this.blocked = blocked;
  }
}

export function check(condition, code, blocked = false) {
  if (!condition) throw new GateError(code, blocked);
}

export const hash = (value) => createHash('sha256').update(value).digest('hex');
export const isHash = (value) => typeof value === 'string' && /^[a-f0-9]{64}$/.test(value);
export const clock = { now: () => performance.now(), sleep: setTimeout };

export function finite(value, code = 'invalid_measurement') {
  check(typeof value === 'number' && Number.isFinite(value) && value >= 0, code);
  return value;
}

export function count(value, code = 'invalid_count') {
  check(Number.isSafeInteger(value) && value >= 0, code);
  return value;
}

export function safeFailure(error) {
  // Adapter and network exceptions can contain URLs, response bodies and credentials.
  return error instanceof GateError && /^[a-z][a-z0-9_]{0,80}$/.test(error.code)
    ? { status: error.blocked ? 'BLOCKED' : 'FAIL', code: error.code }
    : { status: 'FAIL', code: 'adapter_or_transport_error' };
}

export async function withinDeadline(run, timeoutMs, code) {
  const controller = new AbortController();
  let timer;
  const deadline = new Promise((_, reject) => {
    timer = globalThis.setTimeout(() => {
      const error = new GateError(code);
      reject(error);
      controller.abort(error);
    }, timeoutMs);
  });
  try {
    return await Promise.race([run(controller.signal), deadline]);
  } finally {
    globalThis.clearTimeout(timer);
  }
}

export async function waitFor(read, accept, { timeoutMs, intervalMs = 250, time = clock }) {
  finite(timeoutMs); finite(intervalMs);
  check(timeoutMs > 0 && intervalMs > 0, 'invalid_poll_budget');
  const started = time.now();
  while (time.now() - started < timeoutMs) {
    const remaining = timeoutMs - (time.now() - started);
    const result = await read(remaining);
    if (accept(result) && time.now() - started <= timeoutMs) return result;
    await time.sleep(Math.min(intervalMs, Math.max(0, timeoutMs - (time.now() - started))));
  }
  throw new GateError('recovery_deadline_exceeded');
}

export function reconcile(value, { missingIsBlocked = true } = {}) {
  check(value && Array.isArray(value.commands) && value.commands.length > 0, 'oracle_missing', missingIsBlocked);
  check(isHash(value.evidenceHash) && value.independent === true && value.complete === true,
    'oracle_not_independent_or_complete', missingIsBlocked);
  const admitted = new Set();
  for (const row of value.commands) {
    check(isHash(row.commandHash) && !admitted.has(row.commandHash), 'oracle_command_identity_invalid');
    admitted.add(row.commandHash);
    check(row.accepted === true, 'oracle_includes_unaccepted_command');
    check(count(row.executions) === 1, row.executions === 0 ? 'accepted_work_lost' : 'duplicate_execution');
    check(count(row.effects) === 1, row.effects === 0 ? 'accepted_effect_lost' : 'duplicate_effect');
    check(count(row.staleWrites) === 0, 'stale_epoch_write');
    check(row.terminal === true && row.correctOwner === true, 'terminal_or_owner_invalid');
  }
  check(Array.isArray(value.acceptedHashes) && value.acceptedHashes.length === admitted.size
    && new Set(value.acceptedHashes).size === admitted.size
    && value.acceptedHashes.every((id) => admitted.has(id)), 'accepted_set_mismatch');
  return { commands: admitted.size, evidence_hash: value.evidenceHash };
}
