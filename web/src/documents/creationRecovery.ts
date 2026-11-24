import type { CreationRequest } from './CreationRequestHelp';

export interface CreationRecovery {
  readonly request: CreationRequest;
  readonly uncertain: boolean;
  readonly status?: number;
}

const storageError =
  'Document creation recovery needs browser session storage. Restore it before continuing.';

function storageKey(viewer: string) {
  return `cma:document-creation:${JSON.stringify([location.origin, viewer])}`;
}

export function readCreationRecovery(viewer: string): {
  recovery?: CreationRecovery;
  error?: string;
} {
  try {
    const raw = sessionStorage.getItem(storageKey(viewer));
    if (raw === null) return {};
    const { version, recovery } = JSON.parse(raw) as {
      version?: number;
      recovery?: CreationRecovery;
    };
    const request = recovery?.request;
    if (
      version !== 1 ||
      !request ||
      typeof request.title !== 'string' ||
      !request.title.length ||
      request.title.trim() !== request.title ||
      new TextEncoder().encode(request.title).length > 240 ||
      typeof request.key !== 'string' ||
      !/^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/.test(request.key) ||
      typeof request.startedAt !== 'string' ||
      new Date(request.startedAt).toISOString() !== request.startedAt ||
      typeof recovery.uncertain !== 'boolean' ||
      (recovery.status !== undefined &&
        (!Number.isInteger(recovery.status) || recovery.status < 400 || recovery.status > 599)) ||
      (!recovery.uncertain && !creationRefused(recovery.status))
    ) {
      return { error: storageError };
    }
    return { recovery };
  } catch {
    return { error: storageError };
  }
}

export function writeCreationRecovery(
  viewer: string,
  recovery?: CreationRecovery,
): string | undefined {
  try {
    const key = storageKey(viewer);
    if (recovery) sessionStorage.setItem(key, JSON.stringify({ version: 1, recovery }));
    else sessionStorage.removeItem(key);
  } catch {
    return storageError;
  }
}

export function creationRefused(status?: number): boolean {
  return status !== undefined && status >= 400 && status < 500 && status !== 408;
}
