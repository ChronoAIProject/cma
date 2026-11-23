import { afterEach, expect, it, vi } from 'vitest';
import { readTopographicPalette } from './topographicPalette';

afterEach(() => vi.restoreAllMocks());
function computed(line: string, accent: string, tokens = true) {
  vi.spyOn(window, 'getComputedStyle').mockImplementation(
    (node) =>
      ({
        color: (node as HTMLElement).style.color.includes('border') ? line : accent,
        getPropertyValue: () => (tokens ? 'resolved' : ''),
      }) as unknown as CSSStyleDeclaration,
  );
}
it('reads the two resolved CMA tokens and removes every temporary probe', () => {
  const host = document.createElement('div');
  computed('rgb(128, 64, 32)', 'rgb(200, 245, 66)');
  expect(readTopographicPalette(host)).toEqual({
    line: [128 / 255, 64 / 255, 32 / 255],
    accent: [200 / 255, 245 / 255, 66 / 255],
  });
  expect(host.children).toHaveLength(0);
});
it.each(['', 'transparent', 'rgba(1, 2, 3, .2)', 'rgb(300, 0, 0)', 'color(display-p3 1 0 0)'])(
  'falls back for unvalidated color %s',
  (color) => {
    const host = document.createElement('div');
    computed(color, 'rgb(0, 0, 0)');
    expect(readTopographicPalette(host)).toBeNull();
    expect(host.children).toHaveLength(0);
  },
);
it('requires actual theme tokens instead of accepting inherited fallback colors', () => {
  computed('rgb(0, 0, 0)', 'rgb(0, 0, 0)', false);
  expect(readTopographicPalette(document.createElement('div'))).toBeNull();
});
it('cleans a probe after a browser color resolution error', () => {
  const host = document.createElement('div');
  computed('rgb(0, 0, 0)', 'rgb(0, 0, 0)');
  vi.mocked(getComputedStyle)
    .mockImplementationOnce(
      () => ({ getPropertyValue: () => 'resolved' }) as unknown as CSSStyleDeclaration,
    )
    .mockImplementationOnce(() => {
      throw new Error('synthetic CSS failure');
    });
  expect(() => readTopographicPalette(host)).toThrow('synthetic CSS failure');
  expect(host.children).toHaveLength(0);
});
