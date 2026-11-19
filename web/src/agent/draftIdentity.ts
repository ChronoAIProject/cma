import { agentDraftPrefix, clearOwnerDrafts } from './draftStorage';

const marker = 'cma:agent-draft-owner';
let isolated = true;

export function requireDraftIdentity() {
  if (!isolated)
    throw new Error(
      'Agent draft storage could not be isolated. Restore browser storage and sign in again.',
    );
}

/** Only confirmed identity transitions call this; temporary read errors preserve storage. */
export function retainAgentDraftOwner(owner?: string) {
  try {
    const previous = sessionStorage.getItem(marker);
    if (!isolated || !owner || previous !== owner) {
      sessionStorage.removeItem(marker);
      if (previous) clearOwnerDrafts(previous);
      const keys = Array.from({ length: sessionStorage.length }, (_, index) =>
        sessionStorage.key(index),
      );
      for (const key of keys) if (key?.startsWith(agentDraftPrefix)) sessionStorage.removeItem(key);
    }
    if (owner) sessionStorage.setItem(marker, owner);
    isolated = true;
  } catch {
    isolated = false;
  }
}
