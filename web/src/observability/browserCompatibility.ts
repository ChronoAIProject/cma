/**
 * Privacy-preserving counters for retiring historical browser-storage readers.
 *
 * Every request path is a compile-time literal. Callers can report only a
 * bounded tuple, and neither storage values nor browser/user identifiers ever
 * enter this module. Nginx turns the empty POST into a structured counter and
 * answers directly, so measurement is independent of the control-plane API.
 */

// Outcome suffixes stay short enough for the production Nginx image's fixed
// map hash bucket. Nginx expands every exact path to the full tuple in its log.
export const BROWSER_COMPATIBILITY_COUNTERS = [
  {
    tuple: ['appearance', 'canonical', 'read-ok'],
    path: '/__bc/v1/appearance/canonical/ro',
  },
  {
    tuple: ['appearance', 'canonical', 'read-blocked'],
    path: '/__bc/v1/appearance/canonical/rb',
  },
  {
    tuple: ['appearance', 'canonical', 'write-ok'],
    path: '/__bc/v1/appearance/canonical/wo',
  },
  {
    tuple: ['appearance', 'canonical', 'write-blocked'],
    path: '/__bc/v1/appearance/canonical/wb',
  },
  {
    tuple: ['appearance', 'historical-key', 'fallback-used'],
    path: '/__bc/v1/appearance/historical-key/fu',
  },
  {
    tuple: ['terminal-theme', 'canonical', 'read-ok'],
    path: '/__bc/v1/terminal-theme/canonical/ro',
  },
  {
    tuple: ['terminal-theme', 'canonical', 'read-blocked'],
    path: '/__bc/v1/terminal-theme/canonical/rb',
  },
  {
    tuple: ['terminal-theme', 'canonical', 'write-ok'],
    path: '/__bc/v1/terminal-theme/canonical/wo',
  },
  {
    tuple: ['terminal-theme', 'canonical', 'write-blocked'],
    path: '/__bc/v1/terminal-theme/canonical/wb',
  },
  {
    tuple: ['terminal-theme', 'historical-key', 'fallback-used'],
    path: '/__bc/v1/terminal-theme/historical-key/fu',
  },
  {
    tuple: ['terminal-theme', 'historical-id', 'normalized'],
    path: '/__bc/v1/terminal-theme/historical-id/n',
  },
  {
    tuple: ['view-mode', 'canonical', 'read-ok'],
    path: '/__bc/v1/view-mode/canonical/ro',
  },
  {
    tuple: ['view-mode', 'canonical', 'read-blocked'],
    path: '/__bc/v1/view-mode/canonical/rb',
  },
  {
    tuple: ['view-mode', 'canonical', 'write-ok'],
    path: '/__bc/v1/view-mode/canonical/wo',
  },
  {
    tuple: ['view-mode', 'canonical', 'write-blocked'],
    path: '/__bc/v1/view-mode/canonical/wb',
  },
  {
    tuple: ['view-mode', 'historical-key', 'fallback-used'],
    path: '/__bc/v1/view-mode/historical-key/fu',
  },
  {
    tuple: ['pending-turn', 'canonical', 'read-ok'],
    path: '/__bc/v1/pending-turn/canonical/ro',
  },
  {
    tuple: ['pending-turn', 'canonical', 'read-blocked'],
    path: '/__bc/v1/pending-turn/canonical/rb',
  },
  {
    tuple: ['pending-turn', 'canonical', 'write-ok'],
    path: '/__bc/v1/pending-turn/canonical/wo',
  },
  {
    tuple: ['pending-turn', 'canonical', 'write-blocked'],
    path: '/__bc/v1/pending-turn/canonical/wb',
  },
  {
    tuple: ['pending-turn', 'canonical', 'clear-ok'],
    path: '/__bc/v1/pending-turn/canonical/co',
  },
  {
    tuple: ['pending-turn', 'canonical', 'clear-blocked'],
    path: '/__bc/v1/pending-turn/canonical/cb',
  },
  {
    tuple: ['pending-turn', 'historical-key', 'consumed'],
    path: '/__bc/v1/pending-turn/historical-key/c',
  },
  {
    tuple: ['pending-turn', 'historical-key', 'clear-ok'],
    path: '/__bc/v1/pending-turn/historical-key/co',
  },
  {
    tuple: ['pending-turn', 'historical-key', 'clear-blocked'],
    path: '/__bc/v1/pending-turn/historical-key/cb',
  },
  {
    tuple: ['rail-collapsed', 'canonical', 'read-ok'],
    path: '/__bc/v1/rail-collapsed/canonical/ro',
  },
  {
    tuple: ['rail-collapsed', 'canonical', 'read-blocked'],
    path: '/__bc/v1/rail-collapsed/canonical/rb',
  },
  {
    tuple: ['rail-collapsed', 'canonical', 'write-ok'],
    path: '/__bc/v1/rail-collapsed/canonical/wo',
  },
  {
    tuple: ['rail-collapsed', 'canonical', 'write-blocked'],
    path: '/__bc/v1/rail-collapsed/canonical/wb',
  },
  {
    tuple: ['rail-collapsed', 'historical-key', 'fallback-used'],
    path: '/__bc/v1/rail-collapsed/historical-key/fu',
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
  try {
    if (!Array.isArray(value) || value.length !== 3) {
      return null;
    }

    return (
      BROWSER_COMPATIBILITY_COUNTERS.find((counter) =>
        counter.tuple.every((part, index) => part === value[index]),
      ) ?? null
    );
  } catch {
    // The inline global is untrusted input, including a possible Proxy.
    return null;
  }
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
  let queued: unknown;
  try {
    queued = window.__cmaBrowserCompatibilityQueue;
  } catch {
    return;
  }

  try {
    delete window.__cmaBrowserCompatibilityQueue;
  } catch {
    try {
      window.__cmaBrowserCompatibilityQueue = undefined;
    } catch {
      // A hostile accessor must not affect application startup.
    }
  }

  if (!Array.isArray(queued)) {
    return;
  }

  try {
    const limit = Math.min(queued.length, BROWSER_COMPATIBILITY_COUNTERS.length);
    for (let index = 0; index < limit; index += 1) {
      const counter = counterFor(queued[index]);
      if (counter !== null) {
        emit(counter);
      }
    }
  } catch {
    // Measurement never trusts queue iteration enough to block application boot.
  }
}
