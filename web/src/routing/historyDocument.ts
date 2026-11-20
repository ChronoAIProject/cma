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

/** Run before mounting: legacy entries must unload this document, not emit an unknowable popstate. */
export function prepareHistoryDocument(
  replace = (destination: string) => window.location.replace(destination),
): boolean {
  const url = new URL(window.location.href);
  const handoff = url.searchParams.get(handoffParameter);
  if (handoff) {
    const key = handoffPrefix + handoff;
    const stored = sessionStorage.getItem(key);
    if (stored !== null) {
      const { state, destination } = JSON.parse(stored) as {
        state: Record<string, unknown> | null;
        destination: string;
      };
      const expected = new URL(destination);
      expected.searchParams.set(handoffParameter, handoff);
      if (url.href !== expected.href) throw new Error('History preparation destination changed.');
      window.history.replaceState(
        {
          ...state,
          cmaNavigation: { session: crypto.randomUUID(), index: 0, documentBoundary: true },
        },
        '',
        destination,
      );
      sessionStorage.removeItem(key);
      return true;
    }
  }
  if (
    browserHistoryIndex() !== undefined ||
    window.history.state?.cmaNavigation?.documentBoundary === true
  )
    return true;

  // A distinct URL creates a new Document; replacing/reloading the identical URL does not.
  // Carry only existing history state across that replacement, never an editor draft.
  if (!jsonState(window.history.state)) throw new Error('History state cannot be retained.');
  for (const key of Object.keys(sessionStorage))
    if (key.startsWith(handoffPrefix)) sessionStorage.removeItem(key);
  const id = crypto.randomUUID();
  sessionStorage.setItem(
    handoffPrefix + id,
    JSON.stringify({ state: window.history.state, destination: url.href }),
  );
  url.searchParams.set(handoffParameter, id);
  replace(url.href);
  return false;
}
