import { afterEach, describe, expect, it, vi } from 'vitest';
import { loadSurfaceMode, storeSurfaceMode, SURFACE_MODE_KEY } from './surfaceMode';

afterEach(() => {
  window.localStorage.clear();
  vi.restoreAllMocks();
});

describe('the persisted session surface', () => {
  it('round-trips only the two canonical values', () => {
    storeSurfaceMode('chat');
    expect(window.localStorage.getItem(SURFACE_MODE_KEY)).toBe('chat');
    expect(loadSurfaceMode()).toBe('chat');

    storeSurfaceMode('agent');
    expect(loadSurfaceMode()).toBe('agent');
  });

  it.each([null, '', 'terminal', 'CHAT', '{"mode":"chat"}'])(
    'falls back to Agent for %s',
    (value) => {
      if (value !== null) window.localStorage.setItem(SURFACE_MODE_KEY, value);
      expect(loadSurfaceMode()).toBe('agent');
    },
  );

  it('keeps working when browser storage throws on read or write', () => {
    const read = vi.spyOn(window.localStorage, 'getItem').mockImplementation(() => {
      throw new DOMException('blocked');
    });
    expect(loadSurfaceMode()).toBe('agent');
    expect(read).toHaveBeenCalledWith(SURFACE_MODE_KEY);
    vi.restoreAllMocks();

    const write = vi.spyOn(window.localStorage, 'setItem').mockImplementation(() => {
      throw new DOMException('full');
    });
    expect(() => storeSurfaceMode('chat')).not.toThrow();
    expect(write).toHaveBeenCalledWith(SURFACE_MODE_KEY, 'chat');
  });
});
