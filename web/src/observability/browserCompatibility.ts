/**
 * Privacy-preserving counters for retiring historical browser-storage readers.
 *
 * Every request path is a compile-time literal. Callers can report only a
 * bounded tuple, and neither storage values nor browser/user identifiers ever
 * enter this module. Nginx turns the empty POST into a structured counter and
 * answers directly, so measurement is independent of the control-plane API.
 */

export const BROWSER_COMPATIBILITY_COUNTERS = [
  {
    tuple: ['appearance', 'canonical', 'read-ok'],
    path: '/__browser-compat/v1/appearance/canonical/read-ok',
  },
  {
    tuple: ['appearance', 'canonical', 'read-blocked'],
    path: '/__browser-compat/v1/appearance/canonical/read-blocked',
  },
  {
    tuple: ['appearance', 'canonical', 'write-ok'],
    path: '/__browser-compat/v1/appearance/canonical/write-ok',
  },
  {
    tuple: ['appearance', 'canonical', 'write-blocked'],
    path: '/__browser-compat/v1/appearance/canonical/write-blocked',
  },
  {
    tuple: ['appearance', 'historical-key', 'fallback-used'],
    path: '/__browser-compat/v1/appearance/historical-key/fallback-used',
  },
  {
    tuple: ['terminal-theme', 'canonical', 'read-ok'],
    path: '/__browser-compat/v1/terminal-theme/canonical/read-ok',
  },
  {
    tuple: ['terminal-theme', 'canonical', 'read-blocked'],
    path: '/__browser-compat/v1/terminal-theme/canonical/read-blocked',
  },
  {
    tuple: ['terminal-theme', 'canonical', 'write-ok'],
    path: '/__browser-compat/v1/terminal-theme/canonical/write-ok',
  },
  {
    tuple: ['terminal-theme', 'canonical', 'write-blocked'],
    path: '/__browser-compat/v1/terminal-theme/canonical/write-blocked',
  },
  {
    tuple: ['terminal-theme', 'historical-key', 'fallback-used'],
    path: '/__browser-compat/v1/terminal-theme/historical-key/fallback-used',
  },
  {
    tuple: ['terminal-theme', 'historical-id', 'normalized'],
    path: '/__browser-compat/v1/terminal-theme/historical-id/normalized',
  },
  {
    tuple: ['view-mode', 'canonical', 'read-ok'],
    path: '/__browser-compat/v1/view-mode/canonical/read-ok',
  },
  {
    tuple: ['view-mode', 'canonical', 'read-blocked'],
    path: '/__browser-compat/v1/view-mode/canonical/read-blocked',
  },
  {
    tuple: ['view-mode', 'canonical', 'write-ok'],
    path: '/__browser-compat/v1/view-mode/canonical/write-ok',
  },
  {
    tuple: ['view-mode', 'canonical', 'write-blocked'],
    path: '/__browser-compat/v1/view-mode/canonical/write-blocked',
  },
  {
    tuple: ['view-mode', 'historical-key', 'fallback-used'],
    path: '/__browser-compat/v1/view-mode/historical-key/fallback-used',
  },
  {
    tuple: ['pending-turn', 'canonical', 'read-ok'],
    path: '/__browser-compat/v1/pending-turn/canonical/read-ok',
  },
  {
    tuple: ['pending-turn', 'canonical', 'read-blocked'],
    path: '/__browser-compat/v1/pending-turn/canonical/read-blocked',
  },
  {
    tuple: ['pending-turn', 'canonical', 'write-ok'],
    path: '/__browser-compat/v1/pending-turn/canonical/write-ok',
  },
  {
    tuple: ['pending-turn', 'canonical', 'write-blocked'],
    path: '/__browser-compat/v1/pending-turn/canonical/write-blocked',
  },
  {
    tuple: ['pending-turn', 'canonical', 'clear-ok'],
    path: '/__browser-compat/v1/pending-turn/canonical/clear-ok',
  },
  {
    tuple: ['pending-turn', 'canonical', 'clear-blocked'],
    path: '/__browser-compat/v1/pending-turn/canonical/clear-blocked',
  },
  {
    tuple: ['pending-turn', 'historical-key', 'consumed'],
    path: '/__browser-compat/v1/pending-turn/historical-key/consumed',
  },
  {
    tuple: ['pending-turn', 'historical-key', 'clear-ok'],
    path: '/__browser-compat/v1/pending-turn/historical-key/clear-ok',
  },
  {
    tuple: ['pending-turn', 'historical-key', 'clear-blocked'],
    path: '/__browser-compat/v1/pending-turn/historical-key/clear-blocked',
  },
  {
    tuple: ['rail-collapsed', 'canonical', 'read-ok'],
    path: '/__browser-compat/v1/rail-collapsed/canonical/read-ok',
  },
  {
    tuple: ['rail-collapsed', 'canonical', 'read-blocked'],
    path: '/__browser-compat/v1/rail-collapsed/canonical/read-blocked',
  },
  {
    tuple: ['rail-collapsed', 'canonical', 'write-ok'],
    path: '/__browser-compat/v1/rail-collapsed/canonical/write-ok',
  },
  {
    tuple: ['rail-collapsed', 'canonical', 'write-blocked'],
    path: '/__browser-compat/v1/rail-collapsed/canonical/write-blocked',
  },
  {
    tuple: ['rail-collapsed', 'historical-key', 'fallback-used'],
    path: '/__browser-compat/v1/rail-collapsed/historical-key/fallback-used',
  },
] as const;

export type BrowserCompatibilityTuple = (typeof BROWSER_COMPATIBILITY_COUNTERS)[number]['tuple'];

declare global {
  interface Window {
    /** Temporary enum-only queue populated by the inline pre-paint script. */
    __cmaBrowserCompatibilityQueue?: unknown;
  }
}

type Counter = (typeof BROWSER_COMPATIBILITY_COUNTERS)[number];

const seen = new Set<string>();

function counterFor(value: unknown): Counter | null {
  if (!Array.isArray(value) || value.length !== 3) {
    return null;
  }

  return (
    BROWSER_COMPATIBILITY_COUNTERS.find((counter) =>
      counter.tuple.every((part, index) => part === value[index]),
    ) ?? null
  );
}

function emit(counter: Counter): void {
  if (seen.has(counter.path)) {
    return;
  }
  // Mark first: retries and duplicate readers must not turn one document into
  // an unbounded counter source, including when the request itself fails.
  seen.add(counter.path);

  try {
    const request = window.fetch(counter.path, {
      method: 'POST',
      credentials: 'omit',
      referrerPolicy: 'no-referrer',
      cache: 'no-store',
      keepalive: true,
    });
    void request.catch(() => {});
  } catch {
    // Compatibility measurement never changes browser-state behavior.
  }
}

/** Record one of the compile-time-enumerated compatibility outcomes. */
export function observeBrowserCompatibility(tuple: BrowserCompatibilityTuple): void {
  const counter = counterFor(tuple);
  if (counter !== null) {
    emit(counter);
  }
}

/**
 * Drain the enum-only pre-paint queue before any bundled appearance read.
 * Unknown entries are discarded and the temporary global never persists.
 */
export function startBrowserCompatibilityMeasurement(): void {
  const queued = window.__cmaBrowserCompatibilityQueue;
  try {
    delete window.__cmaBrowserCompatibilityQueue;
  } catch {
    window.__cmaBrowserCompatibilityQueue = undefined;
  }

  if (!Array.isArray(queued)) {
    return;
  }
  for (const value of queued) {
    const counter = counterFor(value);
    if (counter !== null) {
      emit(counter);
    }
  }
}
