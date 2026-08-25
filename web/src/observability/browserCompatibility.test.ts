import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

async function subject() {
  vi.resetModules();
  return import('./browserCompatibility');
}

describe('browser compatibility observer', () => {
  const fetchMock = vi.fn<typeof fetch>();

  beforeEach(() => {
    fetchMock.mockReset();
    fetchMock.mockResolvedValue({} as Response);
    vi.stubGlobal('fetch', fetchMock);
    delete window.__cmaBrowserCompatibilityQueue;
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it('maps every bounded tuple to one unique literal path', async () => {
    const { BROWSER_COMPATIBILITY_COUNTERS } = await subject();
    const paths = BROWSER_COMPATIBILITY_COUNTERS.map(({ path }) => path);

    expect(BROWSER_COMPATIBILITY_COUNTERS).toHaveLength(30);
    expect(new Set(paths).size).toBe(paths.length);
    for (const { tuple, path } of BROWSER_COMPATIBILITY_COUNTERS) {
      expect(path).toBe(`/__browser-compat/v1/${tuple.join('/')}`);
    }
  });

  it('sends an empty anonymous POST and deduplicates it for the document', async () => {
    const { observeBrowserCompatibility } = await subject();

    observeBrowserCompatibility(['appearance', 'canonical', 'read-ok']);
    observeBrowserCompatibility(['appearance', 'canonical', 'read-ok']);

    expect(fetchMock).toHaveBeenCalledOnce();
    const [path, options] = fetchMock.mock.calls[0] ?? [];
    expect(path).toBe('/__browser-compat/v1/appearance/canonical/read-ok');
    expect(options).toEqual({
      method: 'POST',
      credentials: 'omit',
      referrerPolicy: 'no-referrer',
      cache: 'no-store',
      keepalive: true,
    });
    expect(options).not.toHaveProperty('body');
    expect(options).not.toHaveProperty('headers');
  });

  it('emits every declared tuple only through its fixed mapping', async () => {
    const { BROWSER_COMPATIBILITY_COUNTERS, observeBrowserCompatibility } = await subject();

    for (const counter of BROWSER_COMPATIBILITY_COUNTERS) {
      observeBrowserCompatibility(counter.tuple);
    }

    expect(fetchMock.mock.calls.map(([path]) => path)).toEqual(
      BROWSER_COMPATIBILITY_COUNTERS.map(({ path }) => path),
    );
  });

  it('discards unknown inline entries and deletes the temporary queue', async () => {
    window.__cmaBrowserCompatibilityQueue = [
      ['appearance', 'canonical', 'read-ok'],
      ['appearance', 'canonical', 'read-ok'],
      ['pending-turn', 'historical-key', 'consumed'],
      ['appearance', 'historical-key', 'fallback-used', 'stored-value'],
      ['appearance', 'historical-key', 'write-ok'],
      { key: 'appearance', generation: 'canonical', outcome: 'read-ok' },
      'not-a-tuple',
    ];
    const { observeBrowserCompatibility, startBrowserCompatibilityMeasurement } = await subject();

    startBrowserCompatibilityMeasurement();
    observeBrowserCompatibility(['appearance', 'canonical', 'read-ok']);

    expect(fetchMock.mock.calls.map(([path]) => path)).toEqual([
      '/__browser-compat/v1/appearance/canonical/read-ok',
      '/__browser-compat/v1/pending-turn/historical-key/consumed',
    ]);
    expect('__cmaBrowserCompatibilityQueue' in window).toBe(false);
  });

  it('does not let synchronous or asynchronous request failure affect callers', async () => {
    fetchMock.mockImplementationOnce(() => {
      throw new TypeError('fetch unavailable');
    });
    fetchMock.mockImplementationOnce(() => Promise.reject(new TypeError('offline')));
    const { observeBrowserCompatibility } = await subject();

    expect(() => observeBrowserCompatibility(['appearance', 'canonical', 'read-ok'])).not.toThrow();
    expect(() =>
      observeBrowserCompatibility(['appearance', 'canonical', 'write-ok']),
    ).not.toThrow();
    await Promise.resolve();
  });

  it('drains the pre-paint queue before the bundled appearance reader starts', () => {
    const main = readFileSync(resolve(process.cwd(), 'src/main.tsx'), 'utf8');

    expect(main.indexOf('startBrowserCompatibilityMeasurement();')).toBeGreaterThan(-1);
    expect(main.indexOf('startBrowserCompatibilityMeasurement();')).toBeLessThan(
      main.indexOf('startAppearance();'),
    );
  });
});
