import type { ResourceRevocationStatus } from '@cma/client';
import type { ResourceViewsClient } from './client';
import {
  forgetRevocation,
  readRevocations,
  saveRevocation,
  type RevocationIntent,
} from './revocationIntent';
import {
  forgetShareChange,
  matchesShareChange,
  readShareChanges,
  saveShareChange,
  type RetainedShareChange,
} from './retainedShareChange';
import {
  matchesRevocationEvidence,
  assertRevocationScope,
  readRevocationEvidence,
  retainRevocationEvidence,
  type RevocationEvidence,
  type RevocationScope,
} from './revocationEvidence';

const observations = new Map<string, symbol>();
let historyReads = 0;

async function observe<T>(
  client: ResourceViewsClient,
  scope: RevocationScope,
  operation: string,
  signal: AbortSignal,
  apply: (status: ResourceRevocationStatus) => T,
): Promise<T> {
  if (!client.resourceRevocations) throw new Error('Revocation status is unavailable.');
  const key = JSON.stringify([scope.owner, scope.kind, scope.id, operation]);
  const token = Symbol();
  observations.set(key, token);
  let abort = () => {};
  try {
    signal.throwIfAborted();
    assertRevocationScope(scope);
    const cancelled = new Promise<never>((_, reject) => {
      abort = () => reject(new Error('Status observation was interrupted.'));
      signal.addEventListener('abort', abort, { once: true });
    });
    const status = await Promise.race([
      client.resourceRevocations.status(operation, scope.kind, scope.id, { signal }),
      cancelled,
    ]);
    signal.throwIfAborted();
    if (observations.get(key) !== token)
      throw new Error('A newer observation superseded this status read.');
    assertRevocationScope(scope);
    const target =
      status.selector.kind === 'agent' ? status.selector.agent_id : status.selector.sandbox_id;
    if (
      status.operation_id !== operation ||
      status.selector.kind !== scope.kind ||
      target !== scope.id
    )
      throw new Error('Revocation status does not match the original operation.');
    return apply(status);
  } finally {
    signal.removeEventListener('abort', abort);
    if (observations.get(key) === token) observations.delete(key);
  }
}

export function retainPendingOperation(
  scope: RevocationScope,
  intent: RevocationIntent,
  expected: RetainedShareChange | undefined,
) {
  assertRevocationScope(scope);
  if (!intent.operation || !/^[a-f0-9]{64}$/.test(intent.operation))
    throw new Error('The original operation is unavailable.');
  if (expected && scope.kind !== 'sandbox') throw new Error('The share scope changed.');
  const target = { kind: 'sandbox' as const, id: scope.id, name: '' };
  const priorShare = expected
    ? readShareChanges(scope.owner, target).find((row) => row.shareId === intent.shareId)
    : undefined;
  const prior = expected
    ? priorShare
    : readRevocations(scope.owner, scope.id).find((row) => row.shareId === intent.shareId);
  if (!prior) throw new Error('The original saved share change is unavailable.');
  if (
    prior.key !== intent.key ||
    prior.revision !== intent.revision ||
    (prior.operation !== undefined && prior.operation !== intent.operation) ||
    (expected &&
      (!priorShare ||
        priorShare.organization !== expected.organization ||
        priorShare.mode !== expected.mode ||
        priorShare.revoked !== expected.revoked))
  )
    throw new Error('The saved share change has a different identity.');
  if (!prior.operation) {
    if (expected)
      saveShareChange(scope.owner, target, { ...expected, operation: intent.operation });
    else saveRevocation(scope.owner, scope.id, intent);
  }
}

function completePending(
  scope: RevocationScope,
  intent: RevocationIntent,
  expected: RetainedShareChange | undefined,
  status: ResourceRevocationStatus,
) {
  retainPendingOperation(scope, intent, expected);
  retainRevocationEvidence(scope, status);
  if (expected)
    forgetShareChange(scope.owner, { kind: 'sandbox', id: scope.id, name: '' }, expected);
  else forgetRevocation(scope.owner, scope.id, intent.shareId);
}

export function observePendingRevocation(
  client: ResourceViewsClient,
  scope: RevocationScope,
  intent: RevocationIntent,
  expected: RetainedShareChange | undefined,
  signal: AbortSignal,
) {
  if (!intent.operation) throw new Error('The original operation is unavailable.');
  return observe(client, scope, intent.operation, signal, (status) => {
    if (
      status.share_id !== intent.shareId ||
      status.expected_revision !== intent.revision ||
      (status.state === 'effective' &&
        (!status.receipt ||
          (expected
            ? !matchesShareChange(status.receipt, scope, expected)
            : !status.receipt.share.revoked_at)))
    )
      throw new Error('The revocation result did not match this share change.');
    if (status.state === 'effective') {
      try {
        completePending(scope, intent, expected, status);
      } catch {
        return { status, retained: false };
      }
    }
    return { status, retained: true };
  });
}

export async function observeRevocationEffects(
  client: ResourceViewsClient,
  scope: RevocationScope,
  evidence: RevocationEvidence,
  signal: AbortSignal,
) {
  if (historyReads >= 4) throw new Error('Other external request checks are in progress.');
  historyReads++;
  try {
    await observe(client, scope, evidence.operation, signal, (status) => {
      if (!matchesRevocationEvidence(scope, evidence, status))
        throw new Error('The original access change could not be verified.');
      const prior = readRevocationEvidence(scope).find(
        (row) => row.operation === evidence.operation,
      );
      if (prior) retainRevocationEvidence(scope, status);
    });
  } finally {
    historyReads--;
  }
}
