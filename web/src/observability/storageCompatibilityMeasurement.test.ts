import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const inline = /<script>([\s\S]*?)<\/script>/.exec(
  readFileSync(resolve(process.cwd(), 'index.html'), 'utf8'),
)?.[1];

function bootInlineAppearance(): void {
  expect(inline, 'index.html has no inline boot script').toBeDefined();
  new Function('window', 'document', inline ?? '')(window, document);
}

function pending(text: string): string {
  return JSON.stringify({ text, capturedAt: Date.now() });
}

function stubSessionStorage(overrides: Partial<Storage>): void {
  vi.stubGlobal('sessionStorage', {
    length: 0,
    clear: vi.fn(),
    getItem: vi.fn(() => null),
    key: vi.fn(() => null),
    removeItem: vi.fn(),
    setItem: vi.fn(),
    ...overrides,
  } satisfies Storage);
}

describe('storage compatibility measurement', () => {
  const fetchMock = vi.fn<typeof fetch>();

  beforeEach(() => {
    vi.restoreAllMocks();
    vi.resetModules();
    window.localStorage.clear();
    window.sessionStorage.clear();
    delete window.__cmaBrowserCompatibilityQueue;
    fetchMock.mockReset();
    fetchMock.mockResolvedValue({} as Response);
    vi.stubGlobal('fetch', fetchMock);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  function paths(): string[] {
    return fetchMock.mock.calls.map(([path]) => String(path));
  }

  it('measures an old-only browser without putting stored values on the queue or wire', async () => {
    window.localStorage.setItem('chrono-bot:appearance', 'dark');
    window.localStorage.setItem('chrono-bot:terminal-theme', 'chronobot-dark');
    window.localStorage.setItem('chrono-bot:view-mode', 'canvas');
    window.localStorage.setItem('chrono-bot:rail-collapsed', JSON.stringify(['active']));
    window.sessionStorage.setItem('chrono-bot:pending-turn', pending('private prompt text'));

    bootInlineAppearance();
    expect(JSON.stringify(window.__cmaBrowserCompatibilityQueue)).not.toContain('dark');
    expect(JSON.stringify(window.__cmaBrowserCompatibilityQueue)).not.toContain(
      'private prompt text',
    );

    const { startBrowserCompatibilityMeasurement } = await import('./browserCompatibility');
    startBrowserCompatibilityMeasurement();
    const { storedAppearance } = await import('../theme/appearance');
    const { storedThemeId } = await import('../terminal/themes');
    const { loadArrangement } = await import('../sessions/arrangement');
    const { loadCollapsed } = await import('../sessions/groups');
    const { consumePendingTurn } = await import('../auth/pendingTurn');

    expect(storedAppearance()).toBe('dark');
    expect(storedThemeId()).toBe('cma-dark');
    expect(loadArrangement()).toBe('canvas');
    expect(loadCollapsed()).toEqual(new Set(['active']));
    expect(consumePendingTurn()).toBe('private prompt text');

    expect(paths()).toEqual(
      expect.arrayContaining([
        '/__bc/v1/appearance/historical-key/fu',
        '/__bc/v1/terminal-theme/historical-key/fu',
        '/__bc/v1/terminal-theme/historical-id/n',
        '/__bc/v1/view-mode/historical-key/fu',
        '/__bc/v1/rail-collapsed/historical-key/fu',
        '/__bc/v1/pending-turn/historical-key/c',
      ]),
    );
    expect(JSON.stringify(fetchMock.mock.calls)).not.toContain('private prompt text');
    expect(window.sessionStorage.getItem('cma:pending-turn')).toBeNull();
    expect(window.sessionStorage.getItem('chrono-bot:pending-turn')).toBeNull();
  });

  it('reports healthy canonical reads without probing counts for absent historical data', async () => {
    window.localStorage.setItem('cma:appearance', 'light');
    window.localStorage.setItem('cma:terminal-theme', 'dracula');
    window.localStorage.setItem('cma:view-mode', 'list');
    window.localStorage.setItem('cma:rail-collapsed', JSON.stringify(['paused']));
    window.sessionStorage.setItem('cma:pending-turn', pending('canonical prompt'));

    const { storedAppearance } = await import('../theme/appearance');
    const { storedThemeId } = await import('../terminal/themes');
    const { loadArrangement } = await import('../sessions/arrangement');
    const { loadCollapsed } = await import('../sessions/groups');
    const { consumePendingTurn } = await import('../auth/pendingTurn');

    expect(storedAppearance()).toBe('light');
    expect(storedThemeId()).toBe('dracula');
    expect(loadArrangement()).toBe('list');
    expect(loadCollapsed()).toEqual(new Set(['paused']));
    expect(consumePendingTurn()).toBe('canonical prompt');

    for (const key of [
      'appearance',
      'terminal-theme',
      'view-mode',
      'pending-turn',
      'rail-collapsed',
    ]) {
      expect(paths()).toContain(`/__bc/v1/${key}/canonical/ro`);
    }
    expect(paths().some((path) => path.includes('/fu'))).toBe(false);
    expect(paths().some((path) => path.endsWith('/c'))).toBe(false);
    expect(paths().some((path) => path.endsWith('/n'))).toBe(false);
  });

  it('counts only a valid historical fallback after malformed canonical data', async () => {
    window.localStorage.setItem('cma:view-mode', 'split-screen');
    const { loadArrangement } = await import('../sessions/arrangement');

    expect(loadArrangement()).toBe('list');
    expect(paths()).not.toContain('/__bc/v1/view-mode/historical-key/fu');

    window.localStorage.setItem('chrono-bot:view-mode', 'canvas');
    expect(loadArrangement()).toBe('canvas');
    expect(paths()).toContain('/__bc/v1/view-mode/historical-key/fu');
  });

  it('reports blocked reads for all five canonical readers without changing fallbacks', async () => {
    const localRead = vi.spyOn(window.localStorage, 'getItem').mockImplementation(() => {
      throw new DOMException('denied', 'SecurityError');
    });
    const { storedAppearance } = await import('../theme/appearance');
    const { storedThemeId } = await import('../terminal/themes');
    const { DEFAULT_ARRANGEMENT, loadArrangement } = await import('../sessions/arrangement');
    const { loadCollapsed } = await import('../sessions/groups');
    const { consumePendingTurn } = await import('../auth/pendingTurn');

    expect(storedAppearance()).toBe('system');
    expect(storedThemeId()).toBeNull();
    expect(loadArrangement()).toBe(DEFAULT_ARRANGEMENT);
    expect(loadCollapsed()).toBeInstanceOf(Set);

    localRead.mockRestore();
    stubSessionStorage({
      getItem: vi.fn(() => {
        throw new DOMException('denied', 'SecurityError');
      }),
    });
    expect(consumePendingTurn()).toBeNull();

    for (const key of [
      'appearance',
      'terminal-theme',
      'view-mode',
      'pending-turn',
      'rail-collapsed',
    ]) {
      expect(paths(), `observed:\n${paths().join('\n')}`).toContain(`/__bc/v1/${key}/canonical/rb`);
    }
    expect(paths().some((path) => path.includes('/fu'))).toBe(false);
    expect(paths().some((path) => path.endsWith('/c'))).toBe(false);
  });

  it('reports blocked canonical writes without blocking browser behavior', async () => {
    const localWrite = vi.spyOn(window.localStorage, 'setItem').mockImplementation(() => {
      throw new DOMException('quota', 'QuotaExceededError');
    });
    const { setAppearance } = await import('../theme/appearance');
    const { storeThemeId } = await import('../terminal/themes');
    const { saveArrangement } = await import('../sessions/arrangement');
    const { storeCollapsed } = await import('../sessions/groups');
    const { capturePendingTurn } = await import('../auth/pendingTurn');

    expect(() => setAppearance('dark')).not.toThrow();
    expect(() => storeThemeId('dracula')).not.toThrow();
    expect(() => saveArrangement('canvas')).not.toThrow();
    expect(() => storeCollapsed(new Set(['active']))).not.toThrow();

    localWrite.mockRestore();
    stubSessionStorage({
      setItem: vi.fn(() => {
        throw new DOMException('quota', 'QuotaExceededError');
      }),
    });
    expect(capturePendingTurn('still returned')).toBe('still returned');

    for (const key of [
      'appearance',
      'terminal-theme',
      'view-mode',
      'pending-turn',
      'rail-collapsed',
    ]) {
      expect(paths(), `observed:\n${paths().join('\n')}`).toContain(`/__bc/v1/${key}/canonical/wb`);
    }
  });

  it('queues blocked pre-paint outcomes without affecting theme resolution', async () => {
    vi.spyOn(window.localStorage, 'getItem').mockImplementation(() => {
      throw new DOMException('denied', 'SecurityError');
    });
    vi.stubGlobal(
      'matchMedia',
      vi.fn(() => ({ matches: true })),
    );

    expect(() => bootInlineAppearance()).not.toThrow();
    expect(window.__cmaBrowserCompatibilityQueue).toEqual([
      ['appearance', 'canonical', 'read-blocked'],
    ]);

    const { startBrowserCompatibilityMeasurement } = await import('./browserCompatibility');
    startBrowserCompatibilityMeasurement();
    expect(paths()).toEqual(['/__bc/v1/appearance/canonical/rb']);
    expect(document.body).toHaveAttribute('data-ds-dark-theme');
  });
});
