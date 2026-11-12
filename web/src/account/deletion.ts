import { ApiError, type AccountDeletionReceipt } from '@chronoai/cma-sdk';
import type { AccountDeletionClient } from '../api/client';
import type { ProvisioningStep } from '../sandbox/progress';

export interface AccountDeletionRequest {
  readonly key: string;
  readonly accepted: boolean;
}

export const ACCOUNT_DELETION_IN_PROGRESS_EVENT = 'cma:account-deletion-in-progress';

const STEP_ORDER = [
  'retire_sandboxes',
  'release_homes',
  'erase_records',
  'erase_objects',
  'revoke_credentials',
  'verify_no_residue',
] as const;

const STEP_LABELS: Readonly<Record<(typeof STEP_ORDER)[number], string>> = {
  retire_sandboxes: 'Retire sandboxes',
  release_homes: 'Release home storage',
  erase_records: 'Erase records',
  erase_objects: 'Erase document objects',
  revoke_credentials: 'Revoke credentials',
  verify_no_residue: 'Verify',
};

export function accountDeletionStorageKey(viewerId: string): string {
  return `cma:account-deletion:${viewerId}`;
}

function randomKey(): string {
  if (typeof crypto.randomUUID === 'function') return crypto.randomUUID();
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (value) => value.toString(16).padStart(2, '0')).join('');
}

export function createAccountDeletionRequest(): AccountDeletionRequest {
  return { key: randomKey(), accepted: false };
}

export function persistAccountDeletion(viewerId: string, request: AccountDeletionRequest): void {
  try {
    localStorage.setItem(accountDeletionStorageKey(viewerId), JSON.stringify(request));
  } catch {
    // The server remains authoritative when browser storage is unavailable.
  }
}

export function clearAccountDeletion(viewerId: string, key?: string): void {
  try {
    const current = restoreAccountDeletion(viewerId);
    if (key === undefined || current?.key === key)
      localStorage.removeItem(accountDeletionStorageKey(viewerId));
  } catch {
    // Ignore storage failures; cleanup owns the durable request.
  }
}

export function restoreAccountDeletion(viewerId: string): AccountDeletionRequest | undefined {
  try {
    const value = JSON.parse(
      localStorage.getItem(accountDeletionStorageKey(viewerId)) ?? 'null',
    ) as Partial<AccountDeletionRequest> | null;
    if (
      value === null ||
      typeof value.key !== 'string' ||
      value.key.length === 0 ||
      value.key.length > 128 ||
      typeof value.accepted !== 'boolean'
    )
      return undefined;
    return { key: value.key, accepted: value.accepted };
  } catch {
    return undefined;
  }
}

export async function requestAccountDeletion(
  client: AccountDeletionClient,
  request: AccountDeletionRequest,
  signal?: AbortSignal,
): Promise<AccountDeletionReceipt> {
  return client.request({ idempotencyKey: request.key, signal });
}

export function readAccountDeletion(
  client: AccountDeletionClient,
  request: AccountDeletionRequest,
  signal?: AbortSignal,
): Promise<AccountDeletionReceipt> {
  // Keep the key in the caller's retained request even though the public read
  // route identifies the owner from the signed-in session.
  void request;
  return client.status({ signal });
}

export function accountDeletionSteps(receipt?: AccountDeletionReceipt): ProvisioningStep[] {
  const completed = new Set(receipt?.steps.map((step) => step.step));
  const currentStep = receipt?.step === 'inventory' ? undefined : receipt?.step;
  const current = currentStep === undefined ? -1 : STEP_ORDER.indexOf(currentStep);
  return STEP_ORDER.map((step, index) => ({
    label: STEP_LABELS[step],
    status:
      receipt?.status === 'completed' || completed.has(step)
        ? 'Done'
        : index === current
          ? 'In progress'
          : 'Waiting',
  }));
}

export function accountDeletionBlockedCopy(receipt?: AccountDeletionReceipt): string | undefined {
  return receipt?.status === 'blocked'
    ? 'Cleanup is delayed. Our team will handle it. You do not need to submit another request.'
    : undefined;
}

export function accountDeletionReadFailureCopy(): string {
  return 'Could not check cleanup progress. Deletion is not confirmed.';
}

export function isAccountDeletionInProgress(error: unknown): boolean {
  return error instanceof ApiError && error.code === 'account_deletion_in_progress';
}

export function isAccountDeletionNotFound(error: unknown): boolean {
  return (
    error instanceof ApiError &&
    (error.code === 'account_deletion_not_found' || error.status === 404)
  );
}

export function isAccountDeletionUnauthorized(error: unknown): boolean {
  return error instanceof ApiError && error.status === 401;
}

export function accountDeletionSessionEndedCopy(): string {
  return 'Your sign-in session has ended. Sign out to continue.';
}

export function notifyAccountDeletionInProgress(error?: unknown): void {
  if (typeof window === 'undefined') return;
  window.dispatchEvent(
    new CustomEvent(ACCOUNT_DELETION_IN_PROGRESS_EVENT, {
      detail: error,
    }),
  );
}
