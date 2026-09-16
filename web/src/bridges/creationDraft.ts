import type { BridgeCreationTarget, CreateBridgeRequest } from '@chronoai/cma-sdk';

type ResourceOwner = CreateBridgeRequest['owner'];

export interface CreationDraft {
  readonly version: 1;
  readonly key: string;
  readonly target?: BridgeCreationTarget;
  readonly request?: CreateBridgeRequest;
}

export function creationStorageKey(viewer: string, organization?: string) {
  return `cma:bridge-creation:${JSON.stringify([location.origin, viewer, organization ?? null])}`;
}

export function sameOwner(left: ResourceOwner, right: ResourceOwner) {
  return left.kind === right.kind && left.subject === right.subject;
}

export function saveCreationDraft(storageKey: string, draft: CreationDraft) {
  // Only callback/identity/configuration references are retained here, never imported credentials.
  sessionStorage.setItem(storageKey, JSON.stringify(draft));
}

export function loadCreationDraft(storageKey: string, owner: ResourceOwner): CreationDraft {
  const raw = sessionStorage.getItem(storageKey);
  if (raw === null) {
    const draft: CreationDraft = { version: 1, key: crypto.randomUUID() };
    saveCreationDraft(storageKey, draft);
    return draft;
  }
  const value = JSON.parse(raw) as CreationDraft;
  if (
    value?.version !== 1 ||
    typeof value.key !== 'string' ||
    !/^[\x21-\x7e]{1,200}$/.test(value.key) ||
    (value.target !== undefined &&
      (typeof value.target.bridge_id !== 'string' ||
        typeof value.target.callback_url !== 'string' ||
        !sameOwner(value.target.owner, owner))) ||
    (value.request !== undefined &&
      (value.target === undefined || !sameOwner(value.request.owner, owner)))
  )
    throw new Error('Saved bridge setup could not be verified.');
  return value;
}
