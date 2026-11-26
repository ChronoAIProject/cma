import type { StartLaunchRequest } from '@cma/client';

export interface DraftStartCommand {
  readonly key: string;
  readonly body: StartLaunchRequest;
}

export interface DraftCredentialRecovery {
  readonly owner: string;
  readonly confirmedDigest?: string;
  readonly command?: DraftStartCommand;
}

function key(launchId: string) {
  return `cma:draft-credential-recovery:${encodeURIComponent(launchId)}`;
}

export interface DraftRecoverySnapshot {
  readonly recovery: DraftCredentialRecovery | undefined;
  readonly raw: string | null;
}

export class DraftRecoveryChangedError extends Error {}

export function readDraftRecovery(launchId: string): DraftCredentialRecovery | undefined {
  return readDraftRecoverySnapshot(launchId).recovery;
}

export function readDraftRecoverySnapshot(launchId: string): DraftRecoverySnapshot {
  try {
    const raw = sessionStorage.getItem(key(launchId));
    if (raw === null) return { recovery: undefined, raw };
    const value: unknown = JSON.parse(raw);
    if (value === null || typeof value !== 'object' || Array.isArray(value))
      throw new Error('Invalid saved request');
    const row = value as Partial<DraftCredentialRecovery>;
    if (
      typeof row.owner !== 'string' ||
      row.owner === '' ||
      (row.confirmedDigest !== undefined && typeof row.confirmedDigest !== 'string')
    )
      throw new Error('Invalid saved request');
    const command = row.command;
    if (
      command !== undefined &&
      (command === null ||
        typeof command !== 'object' ||
        typeof command.key !== 'string' ||
        command.key === '' ||
        command.body === null ||
        typeof command.body !== 'object' ||
        Array.isArray(command.body) ||
        (command.body.trust_confirmed_digest !== undefined &&
          typeof command.body.trust_confirmed_digest !== 'string') ||
        (command.body.acknowledgements !== undefined &&
          (!Array.isArray(command.body.acknowledgements) ||
            !command.body.acknowledgements.every((item) => typeof item === 'string'))))
    )
      throw new Error('Invalid saved request');
    return {
      raw,
      recovery: { owner: row.owner, confirmedDigest: row.confirmedDigest, command },
    };
  } catch {
    throw new Error('Saved request could not be read');
  }
}

export function assertDraftRecovery(launchId: string, expected: DraftRecoverySnapshot) {
  if (sessionStorage.getItem(key(launchId)) !== expected.raw)
    throw new DraftRecoveryChangedError('Saved request changed');
}

export function replaceDraftRecovery(
  launchId: string,
  expected: DraftRecoverySnapshot,
  recovery: DraftCredentialRecovery,
): DraftRecoverySnapshot {
  assertDraftRecovery(launchId, expected);
  // A new attempt owns completion even when it replays the same command.
  const raw = JSON.stringify({ ...recovery, generation: crypto.randomUUID() });
  sessionStorage.setItem(key(launchId), raw);
  return { recovery, raw };
}

export function clearDraftRecovery(launchId: string, expected: DraftRecoverySnapshot) {
  try {
    if (expected.raw === null) return;
    assertDraftRecovery(launchId, expected);
    sessionStorage.removeItem(key(launchId));
  } catch {
    // A storage failure must not hide an accepted Start result.
  }
}
