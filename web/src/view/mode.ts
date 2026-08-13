/**
 * Which way the console arranges sessions: one at a time, or all at once.
 *
 * The choice is a display preference and nothing else — it changes no session
 * state and reaches no API. It is kept in localStorage rather than
 * sessionStorage because, unlike the pending turn, it is a setting someone
 * expects to survive closing the tab, and it is not user content: knowing
 * somebody prefers a canvas leaks nothing.
 */

export type ViewMode = 'tabs' | 'canvas';

/** Tabs is the historical surface and stays the default for a new browser. */
export const DEFAULT_VIEW_MODE: ViewMode = 'tabs';

const KEY = 'chrono-bot:view-mode';

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

function isViewMode(value: unknown): value is ViewMode {
  return value === 'tabs' || value === 'canvas';
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
export function loadViewMode(): ViewMode {
  const store = storage();
  if (store === null) {
    return DEFAULT_VIEW_MODE;
  }

  let raw: string | null = null;
  try {
    raw = store.getItem(KEY);
  } catch {
    return DEFAULT_VIEW_MODE;
  }

  return isViewMode(raw) ? raw : DEFAULT_VIEW_MODE;
}

/** Persist the preference. Never blocks the mode change on storage working. */
export function saveViewMode(mode: ViewMode): void {
  try {
    storage()?.setItem(KEY, mode);
  } catch {
    // Quota or private-mode failure. The mode still changes for this visit; it
    // simply will not be remembered.
  }
}
