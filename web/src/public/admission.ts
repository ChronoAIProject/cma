import { ApiError, type Launch } from '@cma/client';
import type { PublicClient } from './client';
import { type BrowserIntent, hasCurrentConsent, writeIntent } from './intent';
import { publicCopy } from '../vocabulary/needs';
import { settledAllowanceReset } from '../vocabulary/errors';
import { visitorAllowanceGuidanceEnabled } from './visitorAllowanceFeature';

const pending = new Map<string, { identity: string; result: Promise<Launch> }>();

export class AdmissionRecoveryError extends Error {
  constructor() {
    super(
      'The result of your earlier request is unavailable in this tab. Your original request is saved. No new request has been sent. Contact your CMA administrator to check its result.',
    );
  }
}

function commandIdentity(intent: BrowserIntent): string {
  return JSON.stringify([intent.id, intent.path, intent.publicId, JSON.stringify(intent.body)]);
}

export function isSettledQuotaRefusal(error: unknown): boolean {
  return visitorAllowanceGuidanceEnabled() && settledAllowanceReset(error) !== undefined;
}

export function isUnsettledCommand(error: unknown): boolean {
  if (isSettledQuotaRefusal(error)) return false;
  return (
    !(error instanceof ApiError) ||
    error.isRetryable ||
    error.code === 'command_subject_mismatch' ||
    error.code === 'idempotency_in_flight' ||
    error.code === 'idempotency_recovery_required'
  );
}

/** StrictMode and route remounts attach to the same in-flight command. */
export function admit(client: PublicClient, intent: BrowserIntent, owner: string): Promise<Launch> {
  if ((intent.owner !== undefined && intent.owner !== owner) || (intent.submitted && !intent.owner))
    return Promise.reject(new Error(publicCopy.expired));
  if (intent.recoveryRequired) return Promise.reject(new AdmissionRecoveryError());
  const slot = `${owner}:${intent.key}`;
  const current = pending.get(slot);
  const identity = commandIdentity(intent);
  if (current !== undefined)
    return current.identity === identity
      ? current.result
      : Promise.reject(new AdmissionRecoveryError());
  if (intent.submitted && intent.wireBody !== JSON.stringify(intent.body))
    return Promise.reject(new AdmissionRecoveryError());
  if (!hasCurrentConsent(intent))
    return Promise.reject(
      intent.submitted ? new AdmissionRecoveryError() : new Error(publicCopy.expired),
    );
  writeIntent({ ...intent, owner, submitted: true, wireBody: JSON.stringify(intent.body) });
  const result = client.requests.createOrResumeBound(intent.publicId, intent.body, {
    idempotencyKey: intent.key,
    expectedSubject: owner,
  });
  pending.set(slot, { identity, result });
  void result.catch(() => {
    if (pending.get(slot)?.result === result) pending.delete(slot);
  });
  if (pending.size > 50) pending.delete(pending.keys().next().value!);
  return result;
}
