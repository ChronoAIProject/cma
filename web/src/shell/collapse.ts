/**
 * Whether a collapsible panel is open — docs/Web_Style.md §7.
 *
 * "The choice persists per browser and is validated on read."
 *
 * Kept in localStorage because it is a setting somebody expects to survive
 * closing the tab, and it is not user content: knowing somebody works with the
 * rail collapsed leaks nothing.
 *
 * Web storage is untrusted input. Another script on the origin, an older
 * build, or a newer build the user has since rolled back from may have written
 * it, so anything this build does not recognise falls back to the default
 * rather than reaching the layout as a state it cannot draw.
 */

/** Panels whose collapsed state is remembered. One key each, namespaced. */
export type Panel = 'rail' | 'sheet';

const KEY: Record<Panel, string> = {
  rail: 'chrono-bot:rail-collapsed',
  sheet: 'chrono-bot:sheet-collapsed',
};

function storage(): Storage | null {
  try {
    return window.localStorage;
  } catch {
    // Safari in private mode and embedded webviews throw on access rather than
    // merely being empty. A lost preference is acceptable; a crash is not.
    return null;
  }
}

/**
 * Read the stored state.
 *
 * Expanded is the default and the fallback: a panel that opens closed on a
 * value nobody wrote is a panel whose contents somebody has to go looking for.
 */
export function loadCollapsed(panel: Panel): boolean {
  let raw: string | null = null;
  try {
    raw = storage()?.getItem(KEY[panel]) ?? null;
  } catch {
    return false;
  }
  // Exactly two recognised values. Anything else — including the `"true"` a
  // JSON round trip would leave, and the empty string a cleared key can leave
  // behind — is unrecognised and falls back to expanded.
  return raw === 'collapsed' ? true : false;
}

/** Persist the state. Never blocks collapsing on storage working. */
export function saveCollapsed(panel: Panel, collapsed: boolean): void {
  try {
    storage()?.setItem(KEY[panel], collapsed ? 'collapsed' : 'expanded');
  } catch {
    // Quota or private-mode failure. The panel still collapses for this visit;
    // it simply will not be remembered.
  }
}
