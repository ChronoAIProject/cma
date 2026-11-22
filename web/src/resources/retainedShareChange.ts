import type { ResourceShare, ShareReceipt } from '@cma/client';
import type { ResourceViewsClient } from './client';
import type { ShareTarget } from './sharing';
import { requireTaskStorage } from '../create/taskIdentity';

export interface RetainedShareChange {
  readonly key: string;
  readonly shareId: string;
  readonly revision: number;
  readonly organization: string;
  readonly mode: ResourceShare['mode'];
  readonly revoked: boolean;
  readonly operation?: string;
}
const storageKey = (owner: string, target: ShareTarget) =>
  `cma:resource-revocation:${JSON.stringify([owner, target.kind, target.id])}`;
const mode = (value: unknown) =>
  typeof value === 'string' && ['view', 'view_write', 'org_roles'].includes(value);

export function readShareChanges(
  owner: string,
  target: ShareTarget,
): readonly RetainedShareChange[] {
  requireTaskStorage();
  const raw = sessionStorage.getItem(storageKey(owner, target));
  if (raw === null) return [];
  const parsed: unknown = JSON.parse(raw);
  if (
    !Array.isArray(parsed) ||
    parsed.length > 64 ||
    parsed.some((item: unknown) => {
      if (typeof item !== 'object' || item === null) return true;
      const row = item as Record<string, unknown>;
      return (
        typeof row.key !== 'string' ||
        !/^[\x21-\x7e]{1,255}$/.test(row.key) ||
        typeof row.shareId !== 'string' ||
        row.shareId.length === 0 ||
        row.shareId.length > 128 ||
        typeof row.organization !== 'string' ||
        row.organization.length === 0 ||
        row.organization.length > 256 ||
        typeof row.revision !== 'number' ||
        !Number.isSafeInteger(row.revision) ||
        row.revision < 1 ||
        typeof row.revoked !== 'boolean' ||
        !mode(row.mode) ||
        (row.operation !== undefined &&
          (typeof row.operation !== 'string' || !/^[a-f0-9]{64}$/.test(row.operation)))
      );
    })
  )
    throw new Error(
      'Saved share changes could not be read. Restore session storage before retrying.',
    );
  const rows = parsed as RetainedShareChange[];
  if (
    new Set(rows.map((row) => row.shareId)).size !== rows.length ||
    new Set(rows.map((row) => row.key)).size !== rows.length
  )
    throw new Error('Saved share changes contain conflicting requests.');
  return rows;
}
export function saveShareChange(owner: string, target: ShareTarget, change: RetainedShareChange) {
  const rows = readShareChanges(owner, target);
  const prior = rows.find((row) => row.shareId === change.shareId);
  if (
    prior &&
    (prior.key !== change.key ||
      prior.revision !== change.revision ||
      prior.organization !== change.organization ||
      prior.mode !== change.mode ||
      prior.revoked !== change.revoked ||
      (prior.operation !== undefined && prior.operation !== change.operation))
  )
    throw new Error('Confirm the saved share change before starting another one.');
  const next = [...rows.filter((row) => row.shareId !== change.shareId), change];
  if (next.length > 64) throw new Error('Too many share changes are waiting for confirmation.');
  sessionStorage.setItem(storageKey(owner, target), JSON.stringify(next));
}
export function forgetShareChange(owner: string, target: ShareTarget, change: RetainedShareChange) {
  const rows = readShareChanges(owner, target);
  const prior = rows.find((row) => row.shareId === change.shareId);
  if (prior && prior.key !== change.key)
    throw new Error('The saved share request changed. Check it before continuing.');
  const next = rows.filter((row) => row.shareId !== change.shareId);
  if (next.length) sessionStorage.setItem(storageKey(owner, target), JSON.stringify(next));
  else sessionStorage.removeItem(storageKey(owner, target));
}
export function matchesShareChange(
  receipt: ShareReceipt,
  target: { readonly kind: 'agent' | 'sandbox'; readonly id: string },
  change: Pick<RetainedShareChange, 'shareId' | 'revision' | 'mode' | 'revoked' | 'organization'>,
): boolean {
  const share = receipt.share;
  return (
    share.selector.kind === target.kind &&
    ('agent_id' in share.selector ? share.selector.agent_id : share.selector.sandbox_id) ===
      target.id &&
    share.id === change.shareId &&
    share.revision === change.revision + 1 &&
    share.target_org_id === change.organization &&
    share.mode === change.mode &&
    (share.revoked_at !== null) === change.revoked
  );
}
export function sendShareChange(
  client: ResourceViewsClient,
  target: ShareTarget,
  change: RetainedShareChange,
  signal: AbortSignal,
) {
  const options = { signal, idempotencyKey: change.key };
  return change.revoked
    ? client.resourceAccess.revokeShare(
        target.kind,
        target.id,
        change.shareId,
        change.revision,
        options,
      )
    : client.resourceAccess.updateShare(
        target.kind,
        target.id,
        change.shareId,
        { expected_revision: change.revision, mode: change.mode },
        options,
      );
}
