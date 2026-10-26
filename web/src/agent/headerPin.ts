import { useState } from 'react';

function storageKey(viewerId: string | undefined): string {
  return viewerId === undefined ? 'cma:agent-header:pinned' : `cma:agent-header:${viewerId}:pinned`;
}

/** Whether this viewer pinned the Agent header open. Unreadable storage means not pinned. */
export function storedHeaderPin(viewerId: string | undefined): boolean {
  try {
    return localStorage.getItem(storageKey(viewerId)) === 'true';
  } catch {
    // Blocked or unavailable storage: the header starts collapsed, as for a new viewer.
    return false;
  }
}

/**
 * The Agent header's pin, remembered per viewer in this browser — §13.
 *
 * A failed write still pins the header for this view; it only means the
 * choice will not be there on the next visit, which is the same as storage
 * not existing at all.
 */
export function useHeaderPin(viewerId: string | undefined): [boolean, (pinned: boolean) => void] {
  const [pinned, setPinned] = useState(() => storedHeaderPin(viewerId));
  const change = (next: boolean) => {
    setPinned(next);
    try {
      if (next) localStorage.setItem(storageKey(viewerId), 'true');
      else localStorage.removeItem(storageKey(viewerId));
    } catch {
      // The pin applies to this view only; see above.
    }
  };
  return [pinned, change];
}
