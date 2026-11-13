import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';
import { describe, expect, it, vi } from 'vitest';

const source = readFileSync('public/browser-telemetry.js', 'utf8');
function setup(fail = false) {
  const listeners = new Map<string, (event: unknown) => void>();
  const fetch = vi.fn(() => (fail ? Promise.reject(new Error('offline')) : Promise.resolve({})));
  const window = {
    addEventListener: (name: string, callback: (event: unknown) => void) =>
      listeners.set(name, callback),
    cmaReportFatal: undefined as undefined | ((kind: string) => void),
  };
  class ErrorEvent {}
  runInNewContext(source, { window, fetch, ErrorEvent });
  return { listeners, fetch, window, ErrorEvent };
}
describe('browser fatal diagnostics', () => {
  it('starts before the main module and reports only bounded categories once per page', () => {
    const index = readFileSync('index.html', 'utf8');
    expect(index.indexOf('src="/browser-telemetry.js"')).toBeLessThan(
      index.indexOf('src="/src/main.tsx"'),
    );
    const { window, listeners, fetch, ErrorEvent } = setup();
    listeners.get('error')?.(new ErrorEvent());
    listeners.get('error')?.({ target: { tagName: 'SCRIPT', src: 'https://private/path' } });
    listeners.get('unhandledrejection')?.({ reason: 'private credential' });
    listeners.get('vite:preloadError')?.({ payload: 'private stack' });
    window.cmaReportFatal?.('react_error');
    for (let i = 0; i < 100; i++) window.cmaReportFatal?.('react_error');
    window.cmaReportFatal?.('arbitrary-label');
    expect(fetch).toHaveBeenCalledTimes(5);
    for (const call of fetch.mock.calls as unknown as [string, RequestInit][]) {
      expect(call[0]).toBe('/api/v1/telemetry/browser');
      expect(call[1].credentials).toBe('omit');
      expect(call[1].keepalive).toBe(true);
      expect(Object.keys(JSON.parse(call[1].body as string))).toEqual(['kind']);
      expect(JSON.stringify(call)).not.toContain('private');
    }
  });
  it('does not recursively report rejected network requests or synchronous failures', async () => {
    const { window, fetch } = setup(true);
    window.cmaReportFatal?.('react_error');
    await Promise.resolve();
    await Promise.resolve();
    expect(fetch).toHaveBeenCalledTimes(1);
    const listeners = new Map();
    const isolated = {
      addEventListener: (name: string, callback: unknown) => listeners.set(name, callback),
      cmaReportFatal: undefined as undefined | ((kind: string) => void),
    };
    runInNewContext(source, {
      window: isolated,
      fetch: () => {
        throw Error('offline');
      },
    });
    expect(() => isolated.cmaReportFatal?.('react_error')).not.toThrow();
  });
});
