export const preparationDraftPrefix = 'cma:preparation-draft:v1:';
const marker = 'cma:preparation-draft-owner';
let isolated = true;
let cleanupRequired = false;

export function requirePreparationDraftOwner(owner: string, storage: Storage = sessionStorage) {
  if (!isolated || !owner || storage.getItem(marker) !== owner)
    throw new Error(
      'Preparation draft storage could not be isolated. Restore browser storage and sign in again.',
    );
}

export function retainPreparationDraftOwner(owner?: string) {
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
    ).filter((key): key is string => key?.startsWith(preparationDraftPrefix) === true);
    if (cleanupRequired && (previous !== null || keys.length > 0))
      sessionStorage.setItem(marker, '');
    for (const key of keys) {
      let identity: unknown;
      try {
        identity = JSON.parse(key.slice(preparationDraftPrefix.length));
      } catch {
        /* Invalid namespace keys cannot be attributed to an owner. */
      }
      if (
        cleanupRequired ||
        !Array.isArray(identity) ||
        identity[0] !== location.origin ||
        identity[1] !== owner
      )
        sessionStorage.removeItem(key);
    }
    if (owner) sessionStorage.setItem(marker, owner);
    isolated = true;
    cleanupRequired = false;
  } catch {
    isolated = false;
  }
}
