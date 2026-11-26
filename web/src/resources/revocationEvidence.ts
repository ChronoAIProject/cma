import type { ResourceRevocationStatus, ResourceShare } from '@cma/client';
import { requireTaskStorage } from '../create/taskIdentity';
import { matchesShareChange } from './retainedShareChange';

export interface RevocationScope {
  readonly owner: string;
  readonly kind: 'agent' | 'sandbox';
  readonly id: string;
}

export interface RevocationEvidence {
  readonly operation: string;
  readonly shareId: string;
  readonly revision: number;
  readonly organization: string;
  readonly mode: ResourceShare['mode'];
  readonly revoked: boolean;
}

const changed = 'cma:revocation-evidence-changed';
const limit = 64 * 1024;
const boundedText = (value: unknown, length: number) =>
  typeof value === 'string' && value.length > 0 && value.length <= length;

export function assertRevocationScope(scope: RevocationScope) {
  requireTaskStorage();
  const viewer = sessionStorage.getItem('cma:task-viewer');
  if (
    !boundedText(scope.owner, 1024) ||
    !boundedText(scope.id, 1024) ||
    !['agent', 'sandbox'].includes(scope.kind) ||
    (viewer !== null && viewer !== scope.owner)
  )
    throw new Error('Saved external request status is unavailable for this account.');
}

function storageKey(scope: RevocationScope) {
  assertRevocationScope(scope);
  return `cma:resource-revocation:observed:${JSON.stringify([scope.owner, scope.kind, scope.id])}`;
}

function valid(row: unknown): row is RevocationEvidence {
  if (typeof row !== 'object' || row === null) return false;
  const value = row as Record<string, unknown>;
  return (
    typeof value.operation === 'string' &&
    /^[a-f0-9]{64}$/.test(value.operation) &&
    boundedText(value.shareId, 128) &&
    typeof value.revision === 'number' &&
    Number.isSafeInteger(value.revision) &&
    value.revision >= 1 &&
    boundedText(value.organization, 256) &&
    typeof value.mode === 'string' &&
    ['view', 'view_write', 'org_roles'].includes(value.mode) &&
    typeof value.revoked === 'boolean'
  );
}

export function readRevocationEvidence(scope: RevocationScope): readonly RevocationEvidence[] {
  const raw = sessionStorage.getItem(storageKey(scope));
  if (raw === null) return [];
  if (raw.length > limit || new TextEncoder().encode(raw).length > limit)
    throw new Error('Saved external request status is too large.');
  const parsed: unknown = JSON.parse(raw);
  if (typeof parsed !== 'object' || parsed === null)
    throw new Error('Saved external request status could not be read.');
  const value = parsed as Record<string, unknown>;
  if (
    value.version !== 1 ||
    !Array.isArray(value.records) ||
    value.records.length > 64 ||
    !value.records.every(valid) ||
    new Set(value.records.map((row) => row.operation)).size !== value.records.length
  )
    throw new Error('Saved external request status could not be read.');
  return value.records;
}

function write(scope: RevocationScope, records: readonly RevocationEvidence[]) {
  const key = storageKey(scope);
  if (records.length === 0) sessionStorage.removeItem(key);
  else {
    const raw = JSON.stringify({ version: 1, records });
    if (records.length > 64 || new TextEncoder().encode(raw).length > limit)
      throw new Error('Too many external requests are waiting for confirmation.');
    sessionStorage.setItem(key, raw);
  }
  window.dispatchEvent(new Event(changed));
}

export function matchesRevocationEvidence(
  scope: RevocationScope,
  evidence: RevocationEvidence,
  status: ResourceRevocationStatus,
) {
  return (
    status.operation_id === evidence.operation &&
    status.share_id === evidence.shareId &&
    status.expected_revision === evidence.revision &&
    status.state === 'effective' &&
    status.receipt !== null &&
    status.receipt.operation_id === evidence.operation &&
    Number.isSafeInteger(status.unresolved_effects) &&
    status.unresolved_effects >= 0 &&
    status.unresolved_effects <= 8192 &&
    matchesShareChange(status.receipt, scope, evidence)
  );
}

export function retainRevocationEvidence(scope: RevocationScope, status: ResourceRevocationStatus) {
  const receipt = status.receipt;
  if (!receipt || status.state !== 'effective') throw new Error('Access change is not confirmed.');
  const evidence: RevocationEvidence = {
    operation: status.operation_id,
    shareId: status.share_id,
    revision: status.expected_revision,
    organization: receipt.share.target_org_id,
    mode: receipt.share.mode,
    revoked: receipt.share.revoked_at !== null,
  };
  if (!valid(evidence) || !matchesRevocationEvidence(scope, evidence, status))
    throw new Error('External request status does not match the access change.');
  const records = readRevocationEvidence(scope);
  const prior = records.find((row) => row.operation === evidence.operation);
  if (prior && !matchesRevocationEvidence(scope, prior, status))
    throw new Error('The original access change receipt changed.');
  if (status.unresolved_effects === 0) {
    if (prior)
      write(
        scope,
        records.filter((row) => row.operation !== evidence.operation),
      );
  } else if (!prior) write(scope, [...records, evidence]);
}

export function subscribeRevocationEvidence(listener: () => void) {
  window.addEventListener(changed, listener);
  window.addEventListener('storage', listener);
  return () => {
    window.removeEventListener(changed, listener);
    window.removeEventListener('storage', listener);
  };
}
