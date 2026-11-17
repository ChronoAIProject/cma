import { ApiError, type Launch } from '@cma/client';
import type { PublicClient } from './client';
import { type BrowserIntent, writeIntent } from './intent';
import { publicCopy } from '../vocabulary/needs';

const pending = new Map<string, Promise<Launch>>();

export function isUnsettledCommand(error: unknown): boolean {
  return (
    !(error instanceof ApiError) ||
    error.isRetryable ||
    error.code === 'idempotency_in_flight' ||
    error.code === 'idempotency_recovery_required'
  );
}

/** StrictMode and route remounts attach to the same in-flight command. */
export function admit(client: PublicClient, intent: BrowserIntent, owner: string): Promise<Launch> {
  if (intent.expiresAt <= Date.now() || (intent.owner !== undefined && intent.owner !== owner))
    return Promise.reject(new Error(publicCopy.expired));
  const slot = `${owner}:${intent.key}`;
  const current = pending.get(slot);
  if (current !== undefined) return current;
  writeIntent({ ...intent, owner, submitted: true });
  const result = client.requests.createOrResume(intent.publicId, intent.body, {
    idempotencyKey: intent.key,
  });
  pending.set(slot, result);
  void result.catch(() => {
    pending.delete(slot);
  });
  if (pending.size > 50) pending.delete(pending.keys().next().value!);
  return result;
}
