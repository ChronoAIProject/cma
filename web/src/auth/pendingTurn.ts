/**
 * Carries the visitor's typed prompt across the sign-in round trip.
 *
 * The landing terminal accepts a real prompt before the visitor has an
 * identity. Their text must survive the redirect to NyxID and back, so it is
 * held here.
 *
 * sessionStorage, deliberately, and never a `?next=` query parameter: a URL
 * parameter lands in server access logs, browser history and the Referer
 * header. This text is user content that may describe private work, so putting
 * it in a URL would violate the rule against writing sensitive data to logs.
 * sessionStorage is also per-tab and dies with the tab, so a shared machine
 * does not leak a stranger's prompt.
 *
 * This is user content, never a credential. No token is stored here or
 * anywhere else in web storage.
 */

import { observeBrowserCompatibility } from '../observability/browserCompatibility';

const KEY = 'cma:pending-turn';
const LEGACY_KEY = 'chrono-bot:pending-turn';

/** Matches the REST surface's own bound on a turn. */
export const MAX_TURN_LENGTH = 4000;

export interface PendingTurn {
  readonly text: string;
  /** Epoch milliseconds, used to discard a stale capture. */
  readonly capturedAt: number;
}

/** Discard anything older than this; a day-old prompt is not what they meant. */
const MAX_AGE_MS = 60 * 60 * 1000;

function storage(): Storage | null {
  try {
    return window.sessionStorage;
  } catch {
    // Safari in private mode and embedded webviews can throw on access rather
    // than merely being empty. Losing the prompt is acceptable; crashing the
    // landing page is not.
    return null;
  }
}

function readPendingTurn(
  store: Storage,
  key: string,
  observeCanonical: boolean = false,
): string | null {
  try {
    const value = store.getItem(key);
    if (observeCanonical) {
      observeBrowserCompatibility(['pending-turn', 'canonical', 'read-ok']);
    }
    return value;
  } catch {
    if (observeCanonical) {
      observeBrowserCompatibility(['pending-turn', 'canonical', 'read-blocked']);
    }
    return null;
  }
}

function removePendingTurns(store: Storage): void {
  for (const key of [KEY, LEGACY_KEY]) {
    const generation = key === KEY ? 'canonical' : 'historical-key';
    try {
      store.removeItem(key);
      if (generation === 'canonical') {
        observeBrowserCompatibility(['pending-turn', 'canonical', 'clear-ok']);
      } else {
        observeBrowserCompatibility(['pending-turn', 'historical-key', 'clear-ok']);
      }
    } catch {
      if (generation === 'canonical') {
        observeBrowserCompatibility(['pending-turn', 'canonical', 'clear-blocked']);
      } else {
        observeBrowserCompatibility(['pending-turn', 'historical-key', 'clear-blocked']);
      }
      // Try both names even when one removal is blocked.
    }
  }
}

function parsePendingTurn(rawValue: string | null, now: number): string | null {
  if (rawValue === null) {
    return null;
  }

  // Anything in web storage is untrusted input: another script on the origin,
  // or an older build, may have written it. Validate rather than assume shape.
  try {
    const parsed: unknown = JSON.parse(rawValue);
    if (typeof parsed !== 'object' || parsed === null) {
      return null;
    }
    const { text, capturedAt } = parsed as Partial<PendingTurn>;
    if (typeof text !== 'string' || typeof capturedAt !== 'number') {
      return null;
    }
    if (!Number.isFinite(capturedAt) || now - capturedAt > MAX_AGE_MS) {
      return null;
    }
    const trimmed = text.trim();
    return trimmed.length > 0 ? trimmed.slice(0, MAX_TURN_LENGTH) : null;
  } catch {
    return null;
  }
}

/**
 * Validate and store the visitor's prompt.
 * Returns the normalised text, or null if there was nothing worth keeping.
 */
export function capturePendingTurn(raw: string, now: number = Date.now()): string | null {
  const text = raw.trim();
  if (text.length === 0) {
    return null;
  }

  const clipped = text.slice(0, MAX_TURN_LENGTH);
  const store = storage();
  if (store) {
    try {
      const payload: PendingTurn = { text: clipped, capturedAt: now };
      store.setItem(KEY, JSON.stringify(payload));
      observeBrowserCompatibility(['pending-turn', 'canonical', 'write-ok']);
    } catch {
      observeBrowserCompatibility(['pending-turn', 'canonical', 'write-blocked']);
      // Quota or private-mode failure. The redirect still proceeds; the
      // visitor simply retypes. Never block sign-in on storage.
      return clipped;
    }
  } else {
    observeBrowserCompatibility(['pending-turn', 'canonical', 'write-blocked']);
  }
  return clipped;
}

/**
 * Read and immediately clear the stored prompt, so a page refresh after
 * sign-in cannot resubmit it a second time.
 */
export function consumePendingTurn(now: number = Date.now()): string | null {
  const store = storage();
  if (!store) {
    observeBrowserCompatibility(['pending-turn', 'canonical', 'read-blocked']);
    observeBrowserCompatibility(['pending-turn', 'canonical', 'clear-blocked']);
    observeBrowserCompatibility(['pending-turn', 'historical-key', 'clear-blocked']);
    return null;
  }

  const canonical = readPendingTurn(store, KEY, true);
  const historical = readPendingTurn(store, LEGACY_KEY);
  removePendingTurns(store);

  const canonicalTurn = parsePendingTurn(canonical, now);
  if (canonicalTurn !== null) {
    return canonicalTurn;
  }

  const historicalTurn = parsePendingTurn(historical, now);
  if (historicalTurn !== null) {
    observeBrowserCompatibility(['pending-turn', 'historical-key', 'consumed']);
  }
  return historicalTurn;
}

/** Drop any stored prompt without reading it. */
export function clearPendingTurn(): void {
  const store = storage();
  if (store !== null) {
    removePendingTurns(store);
  } else {
    observeBrowserCompatibility(['pending-turn', 'canonical', 'clear-blocked']);
    observeBrowserCompatibility(['pending-turn', 'historical-key', 'clear-blocked']);
  }
}
