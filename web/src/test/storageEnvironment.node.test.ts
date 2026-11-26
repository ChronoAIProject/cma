// @vitest-environment node
import { expect, it } from 'vitest';

it('leaves node-environment utilities outside the browser storage fixture', () => {
  expect('window' in globalThis).toBe(false);
  expect('jsdom' in globalThis).toBe(false);
  // Native Web Storage may be disabled by the caller; the setup must not add it.
  if ('sessionStorage' in globalThis)
    expect(Object.getPrototypeOf(sessionStorage)).toBe(Storage.prototype);
});
