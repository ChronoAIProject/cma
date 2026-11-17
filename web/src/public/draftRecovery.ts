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

export function readDraftRecovery(launchId: string): DraftCredentialRecovery | undefined {
  try {
    const value: unknown = JSON.parse(sessionStorage.getItem(key(launchId)) ?? 'null');
    if (value === null || typeof value !== 'object' || Array.isArray(value)) return undefined;
    const row = value as Partial<DraftCredentialRecovery>;
    if (
      typeof row.owner !== 'string' ||
      row.owner === '' ||
      (row.confirmedDigest !== undefined && typeof row.confirmedDigest !== 'string')
    )
      return undefined;
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
      return undefined;
    return {
      owner: row.owner,
      confirmedDigest: row.confirmedDigest,
      command:
        command === undefined
          ? undefined
          : {
              key: command.key,
              body: {
                trust_confirmed_digest: command.body.trust_confirmed_digest,
                acknowledgements: command.body.acknowledgements,
              },
            },
    };
  } catch {
    return undefined;
  }
}

export function writeDraftRecovery(launchId: string, recovery: DraftCredentialRecovery) {
  sessionStorage.setItem(key(launchId), JSON.stringify(recovery));
}

export function clearDraftRecovery(launchId: string) {
  try {
    sessionStorage.removeItem(key(launchId));
  } catch {
    // A storage failure must not hide an accepted Start result.
  }
}
