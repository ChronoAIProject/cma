const handoffParameter = 'cmaHistoryDocument';
const handoffPrefix = 'cma.history-document.';

function jsonState(value: unknown, seen = new Set<object>()): boolean {
  if (value === null || ['string', 'boolean'].includes(typeof value)) return true;
  if (typeof value === 'number') return Number.isFinite(value);
  if (typeof value !== 'object' || seen.has(value)) return false;
  if (!Array.isArray(value) && Object.getPrototypeOf(value) !== Object.prototype) return false;
  seen.add(value);
  return Object.values(value).every((child) => jsonState(child, seen));
}

export function browserHistoryIndex(): number | undefined {
  const navigation = (window as Window & { navigation?: { currentEntry?: { index: number } } })
    .navigation;
  const index = navigation?.currentEntry?.index;
  return index !== undefined && index >= 0 ? index : undefined;
}

interface Handoff {
  state: Record<string, unknown> | null;
  destination: string;
}

function matchingHandoff(stored: string, url: URL, id: string): Handoff | undefined {
  let parsed: unknown;
  try {
    parsed = JSON.parse(stored);
  } catch {
    return;
  }
  if (!parsed || typeof parsed !== 'object' || !('state' in parsed) || !('destination' in parsed))
    return;
  const { state, destination } = parsed;
  if (
    (state !== null && (typeof state !== 'object' || Array.isArray(state))) ||
    !jsonState(state) ||
    typeof destination !== 'string'
  )
    return;
  let expected: URL;
  try {
    expected = new URL(destination);
  } catch {
    return;
  }
  if (expected.searchParams.has(handoffParameter)) return;
  expected.searchParams.set(handoffParameter, id);
  if (url.href === expected.href) return { state: state as Handoff['state'], destination };
}

/** Run before mounting: legacy entries must unload this document, not emit an unknowable popstate. */
export function prepareHistoryDocument(
  replace = (destination: string) => window.location.replace(destination),
): boolean {
  const url = new URL(window.location.href);
  const handoff = url.searchParams.get(handoffParameter);
  const indexed =
    browserHistoryIndex() !== undefined ||
    window.history.state?.cmaNavigation?.documentBoundary === true;
  if (handoff) {
    const key = handoffPrefix + handoff;
    const stored = sessionStorage.getItem(key);
    const snapshot = stored === null ? undefined : matchingHandoff(stored, url, handoff);
    if (snapshot) {
      window.history.replaceState(
        {
          ...snapshot.state,
          cmaNavigation: { session: crypto.randomUUID(), index: 0, documentBoundary: true },
        },
        '',
        snapshot.destination,
      );
      sessionStorage.removeItem(key);
      return true;
    }
    if (!jsonState(window.history.state)) throw new Error('History state cannot be retained.');
    console.warn('CMA ignored an unavailable or invalid history handoff.');
    url.searchParams.delete(handoffParameter);
    if (indexed) window.history.replaceState(window.history.state, '', url.href);
  }
  if (indexed) return true;

  // A distinct URL creates a new Document; replacing/reloading the identical URL does not.
  // Carry only existing history state across that replacement, never an editor draft.
  const current = window.history.state;
  if (!jsonState(current)) throw new Error('History state cannot be retained.');
  // Replay has always spread the prior state onto the navigation record.
  const state = current === null ? null : { ...current };
  // A successful replay removes its own key. Other entries may be retained for diagnosis.
  const id = crypto.randomUUID();
  sessionStorage.setItem(handoffPrefix + id, JSON.stringify({ state, destination: url.href }));
  url.searchParams.set(handoffParameter, id);
  replace(url.href);
  return false;
}
