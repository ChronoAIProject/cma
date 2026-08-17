import { afterEach, vi } from 'vitest';
import { cleanup } from '@testing-library/react';
import '@testing-library/jest-dom/vitest';

/*
 * Some suites run without a DOM on purpose. `web/src/agui/` is a protocol
 * client and #116 requires it to be testable without jsdom, so everything
 * below that reaches for `window` is skipped rather than crashing the file
 * before its first assertion.
 */
const hasDom = typeof globalThis.window !== 'undefined';

/**
 * jsdom implements neither matchMedia nor ResizeObserver, and both are load
 * bearing here: the intro branches on prefers-reduced-motion before it starts
 * animating, and the terminal refits on container resize. Without these the
 * component throws on mount rather than failing a meaningful assertion.
 */

let reducedMotion = false;

/** Drive the prefers-reduced-motion branch from a test. */
export function setReducedMotion(value: boolean): void {
  reducedMotion = value;
}

if (hasDom && !window.matchMedia) {
  Object.defineProperty(window, 'matchMedia', {
    writable: true,
    value: (query: string) => ({
      matches: query.includes('prefers-reduced-motion: reduce') ? reducedMotion : false,
      media: query,
      onchange: null,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      // xterm's CoreBrowserService tracks device-pixel-ratio through the
      // deprecated listener API, so omitting these makes Terminal.open() throw.
      addListener: vi.fn(),
      removeListener: vi.fn(),
      dispatchEvent: vi.fn(),
    }),
  });
}

/*
 * jsdom has no canvas implementation and logs a "Not implemented" error for
 * every getContext call. xterm degrades correctly without one, so stub it to
 * keep genuine failures visible in test output.
 */
const canvasProto = hasDom ? globalThis.HTMLCanvasElement?.prototype : undefined;
if (canvasProto) {
  canvasProto.getContext = (() => null) as unknown as typeof canvasProto.getContext;
}

/*
 * jsdom opens at 1024px, which §16 reads as the width where the rail arrives
 * collapsed. That is a real behaviour and it has its own tests; it is not what
 * every other test is about, so the window opens at a desk width and a test
 * that cares sets its own.
 */
if (hasDom) {
  Object.defineProperty(window, 'innerWidth', { configurable: true, writable: true, value: 1440 });
}

if (!globalThis.ResizeObserver) {
  globalThis.ResizeObserver = class {
    observe(): void {}
    disconnect(): void {}
    unobserve(): void {}
  } as unknown as typeof ResizeObserver;
}

/*
 * Node's own `localStorage` global shadows jsdom's, and without a backing file
 * it is an empty object with none of the Storage methods on it. Everything the
 * app stores — the appearance, the terminal palette, the view mode — guards
 * against storage being unavailable and so silently took that branch in every
 * test, which meant no stored preference was ever really exercised.
 *
 * Backed by a Map rather than by the real thing: a test that remembers a
 * choice from the test before it is a test that passes for the wrong reason.
 */
const stored = new Map<string, string>();
if (hasDom) {
  Object.defineProperty(window, 'localStorage', {
    configurable: true,
    value: {
      getItem: (key: string) => stored.get(key) ?? null,
      setItem: (key: string, value: string) => {
        stored.set(key, String(value));
      },
      removeItem: (key: string) => {
        stored.delete(key);
      },
      clear: () => {
        stored.clear();
      },
      key: (index: number) => [...stored.keys()][index] ?? null,
      get length() {
        return stored.size;
      },
    } satisfies Storage,
  });
}

afterEach(() => {
  if (hasDom) {
    cleanup();
  }
  setReducedMotion(false);
  stored.clear();
});
