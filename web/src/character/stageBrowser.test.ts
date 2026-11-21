import { afterEach, describe, expect, it, vi } from 'vitest';

const renderer = vi.hoisted(() => ({
  instances: [] as {
    domElement: HTMLCanvasElement;
    setSize: ReturnType<typeof vi.fn>;
    setViewport: ReturnType<typeof vi.fn>;
    setScissor: ReturnType<typeof vi.fn>;
    render: ReturnType<typeof vi.fn>;
    clear: ReturnType<typeof vi.fn>;
    toneMappingExposure: number;
  }[],
  fail: false,
}));

vi.mock('three', async (importOriginal) => {
  const actual = await importOriginal<typeof import('three')>();
  class FakeRenderer {
    domElement: HTMLCanvasElement;
    toneMappingExposure = 1;
    outputColorSpace = '';
    toneMapping = 0;
    setPixelRatio = vi.fn();
    setClearColor = vi.fn();
    setScissorTest = vi.fn();
    setSize = vi.fn();
    setViewport = vi.fn();
    setScissor = vi.fn();
    render = vi.fn();
    clear = vi.fn();
    constructor(options: { canvas: HTMLCanvasElement }) {
      if (renderer.fail) throw new Error('Error creating WebGL context.');
      this.domElement = options.canvas;
      renderer.instances.push(this);
    }
  }
  return { ...actual, WebGLRenderer: FakeRenderer };
});

const { browserStageEnvironment: env } = await import('./stageBrowser');

afterEach(() => {
  renderer.fail = false;
  renderer.instances.length = 0;
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe('browser stage surface', () => {
  it('is unavailable without WebGL2', () => {
    vi.stubGlobal('WebGL2RenderingContext', undefined);
    expect(env.createSurface()).toBeUndefined();
  });

  it('is unavailable, with a warning, when the renderer cannot start', () => {
    vi.stubGlobal('WebGL2RenderingContext', class {});
    renderer.fail = true;
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    expect(env.createSurface()).toBeUndefined();
    expect(warn).toHaveBeenCalledWith(expect.stringContaining('[character]'), expect.any(Error));
  });

  it('grows its buffer only when a larger character needs it', () => {
    vi.stubGlobal('WebGL2RenderingContext', class {});
    const surface = env.createSurface();
    expect(surface?.ensureSize(48, 48)).toEqual({ width: 48, height: 48 });
    expect(surface?.ensureSize(32, 64)).toEqual({ width: 48, height: 64 });
    expect(surface?.ensureSize(16, 16)).toEqual({ width: 48, height: 64 });
    expect(renderer.instances[0]?.setSize).toHaveBeenCalledTimes(2);
  });

  it('renders each character into the bottom-left corner at its own exposure', () => {
    vi.stubGlobal('WebGL2RenderingContext', class {});
    const surface = env.createSurface();
    const rig = { scene: {}, camera: {}, setAspect: vi.fn() };
    surface?.draw(rig as never, 40, 20, 0.78);
    const instance = renderer.instances[0];
    expect(instance?.setViewport).toHaveBeenCalledWith(0, 0, 40, 20);
    expect(instance?.setScissor).toHaveBeenCalledWith(0, 0, 40, 20);
    expect(instance?.clear).toHaveBeenCalled();
    expect(instance?.toneMappingExposure).toBe(0.78);
    expect(rig.setAspect).toHaveBeenCalledWith(2);
    expect(instance?.render).toHaveBeenCalledWith(rig.scene, rig.camera);
  });

  it('reports a lost context and keeps the browser from discarding it silently', () => {
    vi.stubGlobal('WebGL2RenderingContext', class {});
    const surface = env.createSurface();
    const lost = vi.fn();
    surface?.onLost(lost);
    const event = new Event('webglcontextlost', { cancelable: true });
    renderer.instances[0]?.domElement.dispatchEvent(event);
    expect(lost).toHaveBeenCalledTimes(1);
    expect(event.defaultPrevented).toBe(true);
  });
});

describe('browser stage environment', () => {
  it('reads motion, pixel ratio and document visibility from the browser', () => {
    expect(env.reducedMotion()).toBe(false);
    expect(env.pixelRatio()).toBeGreaterThan(0);
    expect(env.documentVisible()).toBe(true);
  });

  it('schedules and cancels animation frames through the browser', () => {
    const request = vi.spyOn(window, 'requestAnimationFrame').mockReturnValue(7);
    const cancel = vi.spyOn(window, 'cancelAnimationFrame').mockImplementation(() => undefined);
    const callback = vi.fn();
    expect(env.requestFrame(callback)).toBe(7);
    env.cancelFrame(7);
    expect(request).toHaveBeenCalledWith(callback);
    expect(cancel).toHaveBeenCalledWith(7);
  });

  it('watches reduced motion and stops watching on request', () => {
    const media = { matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn() };
    vi.spyOn(window, 'matchMedia').mockReturnValue(media as unknown as MediaQueryList);
    const listener = vi.fn();
    const stop = env.watchReducedMotion(listener);
    expect(media.addEventListener).toHaveBeenCalledWith('change', listener);
    stop();
    expect(media.removeEventListener).toHaveBeenCalledWith('change', listener);
  });

  it('notices theme changes on the body', async () => {
    const listener = vi.fn();
    const stop = env.watchTheme(listener);
    document.body.setAttribute('data-ds-dark-theme', '');
    await Promise.resolve();
    expect(listener).toHaveBeenCalled();
    stop();
    document.body.removeAttribute('data-ds-dark-theme');
  });

  it('notices the document being hidden or shown', () => {
    const listener = vi.fn();
    const stop = env.watchDocumentVisibility(listener);
    document.dispatchEvent(new Event('visibilitychange'));
    stop();
    document.dispatchEvent(new Event('visibilitychange'));
    expect(listener).toHaveBeenCalledTimes(1);
  });

  it('reports whether an element is on screen', () => {
    let report: ((entries: { isIntersecting: boolean }[]) => void) | undefined;
    const disconnect = vi.fn();
    vi.stubGlobal(
      'IntersectionObserver',
      class {
        constructor(callback: typeof report) {
          report = callback;
        }
        observe = vi.fn();
        disconnect = disconnect;
      },
    );
    const listener = vi.fn();
    const stop = env.watchVisibility(document.createElement('span'), listener);
    report?.([{ isIntersecting: false }, { isIntersecting: true }]);
    expect(listener).toHaveBeenCalledWith(true);
    stop();
    expect(disconnect).toHaveBeenCalled();
  });

  it('watches an element for resizes', () => {
    const stop = env.watchResize(document.createElement('span'), vi.fn());
    expect(stop).toBeTypeOf('function');
    stop();
  });

  it('logs warnings and errors with a character prefix', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    const error = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    env.log('warn', 'quiet');
    env.log('error', 'loud', { characters: 3 });
    expect(warn).toHaveBeenCalledWith('[character] quiet');
    expect(error).toHaveBeenCalledWith('[character] loud', { characters: 3 });
  });
});
