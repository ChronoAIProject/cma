import { ApiError, type Launch } from '@chronoai/cma-sdk';
import type { PublicClient } from './client';
import { type BrowserIntent, writeIntent } from './intent';
import { publicCopy } from '../vocabulary/needs';

const pending = new Map<string, Promise<Launch>>();

export function isSettledQuotaRefusal(error: unknown): boolean {
  return (
    error instanceof ApiError &&
    error.status === 429 &&
    error.code === 'resource_quota' &&
    error.extensions.resource === 'visitor_launches' &&
    typeof error.extensions.reset_at === 'string' &&
    !Number.isNaN(Date.parse(error.extensions.reset_at))
  );
}

export function isUnsettledCommand(error: unknown): boolean {
  if (isSettledQuotaRefusal(error)) return false;
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
