/**
 * Light or dark, and remembering which.
 *
 * Web_Style.md specifies both themes in full and defines how they resolve: the
 * light palette lives on bare `:root`, dark is redefined under both a
 * `prefers-color-scheme` query and an explicit `[data-theme]` attribute, so an
 * explicit choice wins in either direction. This module only decides which
 * attribute to set; every colour still comes from that document.
 *
 * Three states, not two. "System" is not the same as light or dark — it is the
 * absence of a choice, and collapsing it into one of them means a person who
 * never expressed a preference gets pinned to whatever the page loaded with.
 */

export type Appearance = 'light' | 'dark' | 'system';

const STORAGE_KEY = 'chrono-bot:appearance';

/** Whether a stored value is one we understand. */
function isAppearance(value: unknown): value is Appearance {
  return value === 'light' || value === 'dark' || value === 'system';
}

/**
 * The preference this browser last recorded.
 *
 * Storage can throw — private modes and blocked cookies both do it — and a
 * theme preference is never worth failing to render the page over.
 */
export function storedAppearance(): Appearance {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    return isAppearance(raw) ? raw : 'system';
  } catch {
    return 'system';
  }
}

/**
 * Apply a preference to the document.
 *
 * "System" removes the attribute rather than computing a value, which is what
 * lets the media query in the stylesheet answer — and means a change to the
 * operating system's setting is picked up live, without this code watching for
 * it.
 */
export function applyAppearance(appearance: Appearance): void {
  const root = document.documentElement;
  if (appearance === 'system') {
    delete root.dataset.theme;
  } else {
    root.dataset.theme = appearance;
  }
}

/** Record a preference, and apply it. */
export function setAppearance(appearance: Appearance): void {
  applyAppearance(appearance);
  try {
    window.localStorage.setItem(STORAGE_KEY, appearance);
  } catch {
    // The preference still applies for this page; it simply will not be
    // remembered. Not worth surfacing.
  }
}

/** What the page is actually showing, resolving "system" against the browser. */
export function effectiveAppearance(appearance: Appearance): 'light' | 'dark' {
  if (appearance !== 'system') {
    return appearance;
  }
  return window.matchMedia?.('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
}

/** The order the control cycles through. */
export const APPEARANCE_ORDER: readonly Appearance[] = ['system', 'light', 'dark'];

/** Human label for a preference. */
export function appearanceLabel(appearance: Appearance): string {
  switch (appearance) {
    case 'light':
      return 'Light';
    case 'dark':
      return 'Dark';
    default:
      return 'System';
  }
}
