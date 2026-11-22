import { requireTaskStorage } from '../create/taskIdentity';

export interface RevocationIntent {
  readonly key: string;
  readonly shareId: string;
  readonly revision: number;
  readonly operation?: string;
}

const prefix = 'cma:resource-revocation:';
const storageKey = (owner: string, agent: string) => `${prefix}${JSON.stringify([owner, agent])}`;

export function readRevocations(owner: string, agent: string): readonly RevocationIntent[] {
  requireTaskStorage();
  const raw = sessionStorage.getItem(storageKey(owner, agent));
  if (raw === null) return [];
  const value: unknown = JSON.parse(raw);
  if (
    !Array.isArray(value) ||
    value.length > 64 ||
    value.some((item: unknown) => {
      if (typeof item !== 'object' || item === null) return true;
      const row = item as Record<string, unknown>;
      return (
        typeof row.key !== 'string' ||
        !/^[\x21-\x7e]{1,255}$/.test(row.key) ||
        typeof row.shareId !== 'string' ||
        row.shareId.length === 0 ||
        row.shareId.length > 128 ||
        typeof row.revision !== 'number' ||
        !Number.isSafeInteger(row.revision) ||
        row.revision < 1 ||
        (row.operation !== undefined &&
          (typeof row.operation !== 'string' || !/^[a-f0-9]{64}$/.test(row.operation)))
      );
    })
  )
    throw new Error('Saved revocation could not be read. Restore session storage before retrying.');
  const rows = value as RevocationIntent[];
  if (new Set(rows.map((row) => row.shareId)).size !== rows.length)
    throw new Error('Saved revocation contains conflicting requests.');
  return rows;
}

export function saveRevocation(owner: string, agent: string, intent: RevocationIntent) {
  const previous = readRevocations(owner, agent);
  const rows = [...previous.filter((row) => row.shareId !== intent.shareId), intent];
  if (rows.length > 64) throw new Error('Too many share changes are waiting for confirmation.');
  sessionStorage.setItem(storageKey(owner, agent), JSON.stringify(rows));
}

export function forgetRevocation(owner: string, agent: string, share: string) {
  const rows = readRevocations(owner, agent).filter((row) => row.shareId !== share);
  if (rows.length === 0) sessionStorage.removeItem(storageKey(owner, agent));
  else sessionStorage.setItem(storageKey(owner, agent), JSON.stringify(rows));
}
