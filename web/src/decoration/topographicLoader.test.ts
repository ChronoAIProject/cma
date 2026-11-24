import { afterEach, expect, it, vi } from 'vitest';

afterEach(() => {
  vi.doUnmock('./vendor/reactdrops-topographic/topographic-surface');
  vi.resetModules();
});

it('loads only on demand and shares one successful factory import', async () => {
  const create = vi.fn();
  const imported = vi.fn(() => ({ createTopographicSurface: create }));
  vi.doMock('./vendor/reactdrops-topographic/topographic-surface', imported);
  const { loadTopographicSurface } = await import('./topographicLoader');
  expect(imported).not.toHaveBeenCalled();
  const first = loadTopographicSurface();
  expect(loadTopographicSurface()).toBe(first);
  expect(await first).toBe(create);
  expect(imported).toHaveBeenCalledTimes(1);
});

it('catches a rejected chunk and never retries it in this document', async () => {
  const imported = vi.fn(() => {
    throw new Error('synthetic missing chunk');
  });
  vi.doMock('./vendor/reactdrops-topographic/topographic-surface', imported);
  const { loadTopographicSurface } = await import('./topographicLoader');
  await expect(loadTopographicSurface()).resolves.toBeNull();
  await expect(loadTopographicSurface()).resolves.toBeNull();
  expect(imported).toHaveBeenCalledTimes(1);
});
