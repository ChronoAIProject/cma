import type {} from 'vitest/jsdom';
import { expect, it, vi } from 'vitest';

it('uses the current jsdom session store with the browser Storage prototype', () => {
  expect(sessionStorage).toBe(jsdom.window.sessionStorage);
  expect(sessionStorage).toBeInstanceOf(Storage);
  expect(Object.getPrototypeOf(sessionStorage)).toBe(Storage.prototype);
  sessionStorage.setItem('environment-fixture', 'value');
  expect(jsdom.window.sessionStorage.getItem('environment-fixture')).toBe('value');
});

it.each([
  { method: 'getItem', invoke: () => sessionStorage.getItem('environment-fixture') },
  { method: 'setItem', invoke: () => sessionStorage.setItem('environment-fixture', 'value') },
  { method: 'removeItem', invoke: () => sessionStorage.removeItem('environment-fixture') },
] as const)(
  'observes browser $method refusal through the shared prototype',
  ({ method, invoke }) => {
    const refusal = new DOMException('Storage unavailable', 'SecurityError');
    const spy = vi.spyOn(Storage.prototype, method).mockImplementation(() => {
      throw refusal;
    });
    try {
      expect(invoke).toThrow(refusal);
    } finally {
      spy.mockRestore();
    }
  },
);

it('starts each case with an empty session store', () => {
  expect(sessionStorage.length).toBe(0);
});
