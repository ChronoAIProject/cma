/**
 * Which way the console arranges sessions: one at a time, or all at once.
 *
 * An arrangement, not a view (docs/Web_Style.md §1): it changes how the middle
 * column is drawn and nothing else, and the app carries exactly two views.
 *
 * **It is not a browser-only preference.** A terminal declares its own
 * measurements to the runtime drawing into it, so choosing the canvas asks
 * every attached session for roughly 47 columns and each agent redraws at that
 * width — and output already wrapped there stays wrapped, in the transcript
 * afterwards as well. §13 records that cost; it must not be described away.
 *
 * Kept in localStorage rather than sessionStorage because, unlike the pending
 * turn, it is a setting someone expects to survive closing the tab, and it is
 * not user content: knowing somebody prefers a canvas leaks nothing.
 */

export type Arrangement = 'list' | 'canvas';

/** The list is the historical surface and stays the default for a new browser. */
export const DEFAULT_ARRANGEMENT: Arrangement = 'list';

const KEY = 'cma:view-mode';
const LEGACY_KEY = 'chrono-bot:view-mode';

function storage(): Storage | null {
  try {
    return window.localStorage;
  } catch {
    // Safari in private mode and embedded webviews throw on access rather than
    // merely being empty. A lost preference is acceptable; a crashed console
    // is not.
    return null;
  }
}

function isArrangement(value: unknown): value is Arrangement {
  return value === 'list' || value === 'canvas';
}

function canonicalArrangement(value: string | null): Arrangement | null {
  if (value === 'tabs') {
    return 'list';
  }
  return isArrangement(value) ? value : null;
}

function readArrangement(store: Storage, key: string): string | null {
  try {
    return store.getItem(key);
  } catch {
    return null;
  }
}

function writeArrangement(store: Storage, arrangement: Arrangement): void {
  try {
    store.setItem(KEY, arrangement);
  } catch {
    // Applying the arrangement does not depend on remembering it.
  }
}

/**
 * Read the stored preference, falling back to the default for anything this
 * build does not recognise.
 *
 * Web storage is untrusted input: another script on the origin, an older build,
 * or a newer build the user has since rolled back from may have written it. A
 * value that is not one of the two modes would otherwise reach the console as a
 * mode that renders nothing.
 */
export function loadArrangement(): Arrangement {
  const store = storage();
  if (store === null) {
    return DEFAULT_ARRANGEMENT;
  }

  const rawCanonical = readArrangement(store, KEY);
  const canonical = canonicalArrangement(rawCanonical);
  if (canonical !== null) {
    if (canonical !== rawCanonical) {
      writeArrangement(store, canonical);
    }
    return canonical;
  }

  const historical = canonicalArrangement(readArrangement(store, LEGACY_KEY));
  if (historical === null) {
    return DEFAULT_ARRANGEMENT;
  }

  writeArrangement(store, historical);
  return historical;
}

/** Persist the preference. Never blocks the change on storage working. */
export function saveArrangement(arrangement: Arrangement): void {
  const store = storage();
  if (store !== null) {
    writeArrangement(store, arrangement);
  }
}
