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

const KEY = 'chrono-bot:pending-turn';

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
    } catch {
      // Quota or private-mode failure. The redirect still proceeds; the
      // visitor simply retypes. Never block sign-in on storage.
      return clipped;
    }
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
    return null;
  }

  let rawValue: string | null = null;
  try {
    rawValue = store.getItem(KEY);
    store.removeItem(KEY);
  } catch {
    return null;
  }

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

/** Drop any stored prompt without reading it. */
export function clearPendingTurn(): void {
  try {
    storage()?.removeItem(KEY);
  } catch {
    // Nothing actionable; the value expires with the tab regardless.
  }
}
