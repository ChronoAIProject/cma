import { afterEach, expect, it, vi } from 'vitest';
import { readPalette } from './characterPalette';

const colors: Record<string, string> = {
  '--dsw-static-lime-400': '#c3fd43',
  '--dsw-static-zinc-950': '#09090b',
  '--dsw-static-zinc-50': '#fafafa',
  '--dsw-static-zinc-800': '#27272a',
  '--dsw-static-zinc-400': '#a1a1aa',
  '--dsw-alias-character-failed': '#dc2626',
};

afterEach(() => {
  document.body.removeAttribute('data-ds-dark-theme');
  vi.restoreAllMocks();
});

it('keeps the lime body, black glasses and studio lighting identical in both themes', () => {
  vi.spyOn(window, 'getComputedStyle').mockImplementation(
    () => ({ getPropertyValue: (key: string) => colors[key] ?? '' }) as CSSStyleDeclaration,
  );
  const host = document.createElement('div');
  const light = readPalette(host, 'brand');
  document.body.setAttribute('data-ds-dark-theme', '');
  const dark = readPalette(host, 'brand');
  expect(light).toEqual(dark);
  expect(light.body.getHexString()).toBe('c3fd43');
  expect(light.lens.getHexString()).toBe('09090b');
  expect(readPalette(host, 'failed').body.getHexString()).toBe('dc2626');
});
