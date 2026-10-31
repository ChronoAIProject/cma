import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { CharacterPalette } from '../landing/characterPalette';
import type { CharacterRig, RigFrame } from './rig';
import {
  createCharacterStage,
  type CharacterOptions,
  type StageEnvironment,
  type StageSurface,
} from './stage';

interface FakeRig extends CharacterRig {
  readonly frames: RigFrame[];
}

function fakeRig(): FakeRig {
  const frames: RigFrame[] = [];
  return {
    frames,
    scene: {} as CharacterRig['scene'],
    camera: {} as CharacterRig['camera'],
    setShadow: vi.fn(),
    applyPalette: vi.fn(),
    applyFrame: (frame) => {
      frames.push(frame);
    },
    setAspect: vi.fn(),
    dispose: vi.fn(),
  };
}

function harness(overrides: Partial<StageEnvironment> = {}) {
  let frame: ((time: number) => void) | undefined;
  let lose: (() => void) | undefined;
  let reduced = false;
  let documentShown = true;
  const visibility = new Map<Element, (visible: boolean) => void>();
  const theme: (() => void)[] = [];
  const rigs: FakeRig[] = [];
  const surface: StageSurface = {
    source: document.createElement('canvas'),
    ensureSize: vi.fn(() => ({ width: 128, height: 96 })),
    draw: vi.fn(),
    onLost: (listener) => {
      lose = listener;
    },
  };
  const env: StageEnvironment = {
    createSurface: () => surface,
    createRig: () => {
      const rig = fakeRig();
      rigs.push(rig);
      return rig;
    },
    readPalette: vi.fn(() => ({}) as CharacterPalette),
    requestFrame: vi.fn((callback: (time: number) => void) => {
      frame = callback;
      return 1;
    }),
    cancelFrame: vi.fn(),
    reducedMotion: () => reduced,
    pixelRatio: () => 2,
    documentVisible: () => documentShown,
    watchReducedMotion: () => () => undefined,
    watchTheme: (listener) => {
      theme.push(listener);
      return () => undefined;
    },
    watchDocumentVisibility: () => () => undefined,
    watchVisibility: (element, listener) => {
      visibility.set(element, listener);
      return () => visibility.delete(element);
    },
    watchResize: () => () => undefined,
    log: vi.fn(),
    ...overrides,
  };
  return {
    env,
    surface,
    rigs,
    stage: createCharacterStage(env),
    /** Run the pending animation frame at `time` milliseconds; false when none was pending. */
    run(time: number) {
      const pending = frame;
      frame = undefined;
      if (pending === undefined) return false;
      pending(time);
      return true;
    },
    pending: () => frame !== undefined,
    lose: () => lose?.(),
    setReduced: (value: boolean) => {
      reduced = value;
    },
    setDocumentShown: (value: boolean) => {
      documentShown = value;
    },
    setVisible: (element: Element, value: boolean) => visibility.get(element)?.(value),
    changeTheme: () => {
      for (const listener of theme) listener();
    },
  };
}

const context = { clearRect: vi.fn(), drawImage: vi.fn() };

function canvas(width = 24, height = 24) {
  const element = document.createElement('canvas');
  Object.defineProperty(element, 'clientWidth', { value: width });
  Object.defineProperty(element, 'clientHeight', { value: height });
  return element;
}

const animated: CharacterOptions = {
  pose: 'thinking',
  still: false,
  shadow: false,
  tone: 'brand',
  phase: 0,
};

beforeEach(() => {
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue(
    context as unknown as CanvasRenderingContext2D,
  );
  context.clearRect.mockClear();
  context.drawImage.mockClear();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('createCharacterStage', () => {
  it('reports unavailability once when no WebGL surface can start', () => {
    const h = harness({ createSurface: () => undefined });
    expect(h.stage.attach(canvas(), animated, vi.fn())).toBeUndefined();
    expect(h.stage.attach(canvas(), animated, vi.fn())).toBeUndefined();
    expect(h.env.log).toHaveBeenCalledTimes(1);
  });

  it('falls back when the target canvas has no 2D context', () => {
    vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue(null);
    const h = harness();
    expect(h.stage.attach(canvas(), animated, vi.fn())).toBeUndefined();
    expect(h.env.log).toHaveBeenCalledWith('warn', expect.stringContaining('no 2D context'));
  });

  it('draws an animated character every frame and copies its corner of the buffer', () => {
    const h = harness();
    const target = canvas();
    h.stage.attach(target, animated, vi.fn());
    expect(h.run(0)).toBe(true);
    expect(h.run(40)).toBe(true);
    expect(h.run(80)).toBe(true);
    expect(h.surface.draw).toHaveBeenCalledTimes(3);
    expect(target.width).toBe(48);
    expect(h.surface.draw).toHaveBeenLastCalledWith(h.rigs[0], 48, 48, expect.any(Number));
    expect(context.drawImage).toHaveBeenLastCalledWith(
      h.surface.source,
      0,
      96 - 48,
      48,
      48,
      0,
      0,
      48,
      48,
    );
    expect(h.pending()).toBe(true);
  });

  it('caps animation at 30fps by waiting out display frames between beats', () => {
    const h = harness();
    h.stage.attach(canvas(), animated, vi.fn());
    h.run(0);
    h.run(16);
    expect(h.surface.draw).toHaveBeenCalledTimes(1);
    expect(h.pending()).toBe(true);
    h.run(34);
    expect(h.surface.draw).toHaveBeenCalledTimes(2);
  });

  it('draws a still character once at its rest moment and stops scheduling', () => {
    const h = harness();
    h.stage.attach(canvas(), { ...animated, still: true }, vi.fn());
    h.run(0);
    expect(h.surface.draw).toHaveBeenCalledTimes(1);
    expect(h.pending()).toBe(false);
  });

  it('holds every character still under reduced motion', () => {
    const h = harness();
    h.setReduced(true);
    h.stage.attach(canvas(), animated, vi.fn());
    h.run(0);
    expect(h.pending()).toBe(false);
    expect(h.rigs[0]?.frames).toHaveLength(1);
  });

  it('skips characters scrolled out of view and redraws them when they return', () => {
    const h = harness();
    const target = canvas();
    h.stage.attach(target, animated, vi.fn());
    h.setVisible(target, false);
    h.run(0);
    expect(h.surface.draw).not.toHaveBeenCalled();
    h.setVisible(target, true);
    h.run(16);
    expect(h.surface.draw).toHaveBeenCalledTimes(1);
  });

  it('waits for a zero-sized canvas without drawing', () => {
    const h = harness();
    h.stage.attach(canvas(0, 0), animated, vi.fn());
    h.run(0);
    expect(h.surface.draw).not.toHaveBeenCalled();
    expect(h.pending()).toBe(true);
  });

  it('does not schedule while the document is hidden', () => {
    const h = harness();
    h.setDocumentShown(false);
    h.stage.attach(canvas(), animated, vi.fn());
    expect(h.pending()).toBe(false);
  });

  it('stops a running frame when the document hides mid-loop', () => {
    const h = harness();
    h.stage.attach(canvas(), animated, vi.fn());
    h.setDocumentShown(false);
    h.run(0);
    expect(h.surface.draw).not.toHaveBeenCalled();
    expect(h.pending()).toBe(false);
  });

  it('eases into a new pose instead of jumping', () => {
    const h = harness();
    const handle = h.stage.attach(canvas(), { ...animated, pose: 'paused' }, vi.fn());
    h.run(0);
    const before = h.rigs[0]?.frames.at(-1);
    handle?.update({ ...animated, pose: 'approval' });
    h.run(40);
    h.run(80);
    const blended = h.rigs[0]?.frames.at(-1);
    expect(blended?.y).toBeGreaterThan(before?.y ?? 0);
    expect(blended?.scale[0]).toBeLessThan(1.05);
  });

  it('plays a one-shot pose to its end, then stops asking for frames', () => {
    const h = harness();
    h.stage.attach(canvas(), { ...animated, pose: 'failed' }, vi.fn());
    let time = 0;
    while (h.run(time) && time < 5000) time += 50;
    expect(time).toBeLessThan(5000);
    expect(h.rigs[0]?.frames.at(-1)?.y).toBeLessThan(0);
  });

  it('repaints every character when the theme changes', () => {
    const h = harness();
    h.stage.attach(canvas(), { ...animated, still: true }, vi.fn());
    h.run(0);
    h.changeTheme();
    expect(h.env.readPalette).toHaveBeenCalledTimes(2);
    h.run(16);
    expect(h.surface.draw).toHaveBeenCalledTimes(2);
  });

  it('wears the requested tone and shadow and follows later changes', () => {
    const h = harness();
    const target = canvas();
    const handle = h.stage.attach(target, { ...animated, tone: 'warning' }, vi.fn());
    expect(h.env.readPalette).toHaveBeenLastCalledWith(target, '--dsw-alias-character-pending');
    handle?.update({ ...animated, tone: 'error', shadow: true });
    expect(h.env.readPalette).toHaveBeenLastCalledWith(target, '--dsw-alias-character-failed');
    expect(h.rigs[0]?.setShadow).toHaveBeenLastCalledWith(true);
  });

  it('falls back when the character cannot be built', () => {
    const h = harness({
      readPalette: () => {
        throw new Error('missing token');
      },
    });
    expect(h.stage.attach(canvas(), animated, vi.fn())).toBeUndefined();
    expect(h.env.log).toHaveBeenCalledWith(
      'error',
      expect.stringContaining('could not be built'),
      expect.any(Error),
    );
  });

  it('keeps drawing when a later palette read fails, and logs it', () => {
    let calls = 0;
    const h = harness({
      readPalette: () => {
        calls += 1;
        if (calls > 1) throw new Error('missing token');
        return {} as CharacterPalette;
      },
    });
    h.stage.attach(canvas(), animated, vi.fn());
    h.changeTheme();
    expect(h.env.log).toHaveBeenCalledWith(
      'error',
      expect.stringContaining('did not resolve'),
      expect.any(Error),
    );
    expect(h.run(0)).toBe(true);
  });

  it('switches every character to its fallback when the context is lost', () => {
    const h = harness();
    const first = vi.fn();
    const second = vi.fn();
    h.stage.attach(canvas(), animated, first);
    h.stage.attach(canvas(), animated, second);
    h.lose();
    expect(first).toHaveBeenCalledTimes(1);
    expect(second).toHaveBeenCalledTimes(1);
    expect(h.stage.count()).toBe(0);
    expect(h.stage.attach(canvas(), animated, vi.fn())).toBeUndefined();
  });

  it('releases a disposed character and ignores later updates', () => {
    const h = harness();
    const handle = h.stage.attach(canvas(), animated, vi.fn());
    expect(h.stage.count()).toBe(1);
    handle?.dispose();
    handle?.dispose();
    handle?.update({ ...animated, pose: 'idle' });
    expect(h.stage.count()).toBe(0);
    expect(h.rigs[0]?.dispose).toHaveBeenCalledTimes(1);
  });
});
