import { agentDraftPrefix, clearOwnerDrafts } from './draftStorage';

const marker = 'cma:agent-draft-owner';
let isolated = true;
let cleanupRequired = false;

export function requireDraftIdentity() {
  if (!isolated)
    throw new Error(
      'Agent draft storage could not be isolated. Restore browser storage and sign in again.',
    );
}

/** Only confirmed identity transitions call this; temporary read errors preserve storage. */
export function retainAgentDraftOwner(owner?: string) {
  if (!owner) cleanupRequired = true;
  try {
    const previous = sessionStorage.getItem(marker);
    cleanupRequired ||= previous !== null && previous !== owner;
    if (owner && previous === owner && !cleanupRequired) {
      isolated = true;
      return;
    }
    const keys = Array.from({ length: sessionStorage.length }, (_, index) =>
      sessionStorage.key(index),
    ).filter((key): key is string => key?.startsWith(agentDraftPrefix) === true);
    // Retry interrupted logout cleanup before exposing a later owner's drafts.
    if (cleanupRequired && (previous !== null || keys.length > 0))
      sessionStorage.setItem(marker, '');
    if (cleanupRequired && previous) clearOwnerDrafts(previous);
    for (const key of keys) {
      let identity: unknown;
      try {
        identity = JSON.parse(key.slice(agentDraftPrefix.length));
      } catch {
        /* Malformed namespace keys cannot belong to the verified owner. */
      }
      if (cleanupRequired || !Array.isArray(identity) || identity[1] !== owner)
        sessionStorage.removeItem(key);
    }
    if (owner) sessionStorage.setItem(marker, owner);
    isolated = true;
    cleanupRequired = false;
  } catch {
    isolated = false;
  }
}
