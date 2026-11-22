import { afterEach, expect, it, vi } from 'vitest';
import { mountCharacter } from './characterScene';

vi.mock('three', async (importOriginal) => {
  const actual = await importOriginal<typeof import('three')>();
  class TestRenderer {
    domElement = document.createElement('canvas');
    setPixelRatio = vi.fn();
    setClearColor = vi.fn();
    dispose = vi.fn();
    forceContextLoss = vi.fn();
  }
  return { ...actual, WebGLRenderer: TestRenderer };
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

it('refreshes the palette only while the character host is attached', () => {
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue({
    createRadialGradient: () => ({ addColorStop: vi.fn() }),
    fillRect: vi.fn(),
  } as unknown as CanvasRenderingContext2D);
  const styles = document.createElement('span').style;
  const getColor = vi.spyOn(styles, 'getPropertyValue');
  const readStyles = vi.spyOn(window, 'getComputedStyle').mockImplementation((host) => {
    getColor.mockReturnValue(host.isConnected ? '#0e61ac' : '');
    return styles;
  });
  vi.spyOn(window, 'requestAnimationFrame').mockReturnValue(1);
  vi.stubGlobal(
    'IntersectionObserver',
    class {
      observe = vi.fn();
      disconnect = vi.fn();
    },
  );
  let applyTheme!: () => void;
  vi.stubGlobal(
    'MutationObserver',
    class {
      constructor(onMutation: () => void) {
        applyTheme = onMutation;
      }
      observe = vi.fn();
      disconnect = vi.fn();
    },
  );

  const host = document.body.appendChild(document.createElement('span'));
  const scene = mountCharacter(host, { state: 'running' });
  try {
    expect(readStyles).toHaveBeenCalledExactlyOnceWith(host);
    applyTheme();
    expect(readStyles).toHaveBeenCalledTimes(2);

    host.remove();
    expect(applyTheme).not.toThrow();
    expect(readStyles).toHaveBeenCalledTimes(2);
  } finally {
    scene.dispose();
    host.remove();
  }
});
