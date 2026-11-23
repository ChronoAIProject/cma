import { StrictMode } from 'react';
import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { TopographicBackdrop as BackdropType } from './TopographicBackdrop';

const mocks = vi.hoisted(() => ({ load: vi.fn(), palette: vi.fn() }));
vi.mock('./topographicLoader', () => ({ loadTopographicSurface: mocks.load }));
vi.mock('./topographicPalette', () => ({ readTopographicPalette: mocks.palette }));
let Backdrop: typeof BackdropType;
let observer: IntersectionObserverCallback;
let resize: ResizeObserverCallback;
let observers: { disconnect: ReturnType<typeof vi.fn> }[];
let reduced: boolean;
let motion: EventTarget;
let factory: ReturnType<typeof vi.fn>;
let draw = vi.fn(() => true);
let dispose: ReturnType<typeof vi.fn>;
let lose: ReturnType<typeof vi.fn>;
let context: ReturnType<typeof vi.spyOn>;

beforeEach(async () => {
  vi.resetModules();
  vi.useFakeTimers({
    toFake: [
      'setTimeout',
      'clearTimeout',
      'requestAnimationFrame',
      'cancelAnimationFrame',
      'performance',
    ],
  });
  vi.stubEnv('VITE_HOME_TOPOGRAPHIC_FLOW_ENABLED', 'true');
  reduced = false;
  observers = [];
  motion = new EventTarget();
  // Distinct media objects keep their query-specific live getter.
  vi.stubGlobal('matchMedia', (query: string) => ({
    get matches() {
      return reduced && query.includes('reduce');
    },
    addEventListener: motion.addEventListener.bind(motion),
    removeEventListener: motion.removeEventListener.bind(motion),
  }));
  vi.stubGlobal(
    'IntersectionObserver',
    class {
      disconnect = vi.fn();
      observe = vi.fn();
      constructor(callback: IntersectionObserverCallback) {
        observer = callback;
        observers.push(this);
      }
    },
  );
  vi.stubGlobal(
    'ResizeObserver',
    class {
      disconnect = vi.fn();
      observe = vi.fn();
      constructor(callback: ResizeObserverCallback) {
        resize = callback;
        observers.push(this);
      }
    },
  );
  vi.spyOn(HTMLElement.prototype, 'clientWidth', 'get').mockReturnValue(560);
  vi.spyOn(HTMLElement.prototype, 'clientHeight', 'get').mockReturnValue(104);
  vi.spyOn(HTMLElement.prototype, 'getClientRects').mockReturnValue([{}] as unknown as DOMRectList);
  mocks.palette.mockReset().mockReturnValue({ line: [0.2, 0.2, 0.2], accent: [0.6, 0.8, 0.1] });
  draw = vi.fn(() => true);
  dispose = vi.fn();
  lose = vi.fn();
  factory = vi.fn(
    (_canvas, _gl, takeFrame: () => { submit: (draw: () => void) => boolean } | null) => ({
      renderFrame: () => takeFrame()?.submit(draw) ?? false,
      dispose,
    }),
  );
  mocks.load.mockReset().mockResolvedValue(factory);
  context = vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue({
    getExtension: () => ({ loseContext: lose }),
  } as unknown as WebGL2RenderingContext);
  vi.spyOn(console, 'warn').mockImplementation(() => {});
  ({ TopographicBackdrop: Backdrop } = await import('./TopographicBackdrop'));
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.unstubAllEnvs();
});
function intersection(container: HTMLElement, isIntersecting = true) {
  const target = container.querySelector('.topographic-backdrop')!;
  act(() =>
    observer([{ target, isIntersecting } as IntersectionObserverEntry], {} as IntersectionObserver),
  );
}
async function advance(milliseconds: number) {
  await act(() => vi.advanceTimersByTimeAsync(milliseconds));
}
async function playing(strict = false) {
  const view = render(
    strict ? (
      <StrictMode>
        <Backdrop placement="landing" />
      </StrictMode>
    ) : (
      <Backdrop placement="landing" />
    ),
  );
  intersection(view.container);
  await advance(100);
  expect(draw).toHaveBeenCalled();
  return view;
}

it.each([undefined, 'false', 'TRUE'])('does no work with flag %s', (flag) => {
  vi.stubEnv('VITE_HOME_TOPOGRAPHIC_FLOW_ENABLED', flag);
  const { container } = render(<Backdrop placement="landing" />);
  expect(container).toBeEmptyDOMElement();
  expect(observers).toHaveLength(0);
  expect(context).not.toHaveBeenCalled();
  expect(mocks.load).not.toHaveBeenCalled();
  expect(vi.getTimerCount()).toBe(0);
});
it.each(['reduced', 'small', 'no-observer', 'focused', 'palette'])(
  'stays static before import for %s',
  (policy) => {
    if (policy === 'reduced') reduced = true;
    if (policy === 'small') vi.stubGlobal('innerWidth', 720);
    if (policy === 'no-observer') vi.stubGlobal('IntersectionObserver', undefined);
    if (policy === 'palette') mocks.palette.mockReturnValue(null);
    const input = document.createElement('input');
    if (policy === 'focused') {
      document.body.append(input);
      input.focus();
    }
    const before = vi.getTimerCount();
    const { container } = render(<Backdrop placement="landing" />);
    input.remove();
    expect(container.querySelector('.topographic-backdrop')).toHaveAttribute('aria-hidden', 'true');
    expect(context).not.toHaveBeenCalled();
    expect(mocks.load).not.toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(before);
  },
);
it('waits for intersection and a painted frame before importing', async () => {
  const { container } = render(<Backdrop placement="landing" />);
  await advance(100);
  expect(mocks.load).not.toHaveBeenCalled();
  intersection(container);
  expect(mocks.load).not.toHaveBeenCalled();
  await advance(100);
  expect(mocks.load).toHaveBeenCalledTimes(1);
  expect(container.querySelector('canvas')).not.toHaveAttribute('hidden');
});
it('caps actual draws and disposes after four seconds under StrictMode', async () => {
  const view = await playing(true);
  await advance(1000);
  expect(draw.mock.calls.length).toBeLessThanOrEqual(33);
  await advance(4000);
  const count = draw.mock.calls.length;
  expect(count).toBeLessThanOrEqual(118);
  expect(dispose).toHaveBeenCalledTimes(1);
  expect(lose).toHaveBeenCalledTimes(1);
  expect(vi.getTimerCount()).toBe(0);
  expect(view.container.querySelector('canvas')).toHaveAttribute('hidden');
  fireEvent(window, new Event('resize'));
  await advance(5000);
  expect(draw).toHaveBeenCalledTimes(count);
  view.unmount();
  const remount = render(<Backdrop placement="landing" />);
  intersection(remount.container);
  await advance(5000);
  expect(factory).toHaveBeenCalledTimes(1);
});
it('stops permanently on input focus and keeps the input usable', async () => {
  await playing();
  const input = document.createElement('textarea');
  document.body.append(input);
  act(() => input.focus());
  input.value = 'Keep my work';
  const count = draw.mock.calls.length;
  await advance(1000);
  expect(draw).toHaveBeenCalledTimes(count);
  expect(dispose).toHaveBeenCalledOnce();
  expect(input.value).toBe('Keep my work');
  input.blur();
  input.remove();
  await advance(1000);
  expect(draw).toHaveBeenCalledTimes(count);
});
it('stops when reduced motion changes and never restarts', async () => {
  await playing();
  reduced = true;
  act(() => motion.dispatchEvent(new Event('change')));
  const count = draw.mock.calls.length;
  reduced = false;
  act(() => motion.dispatchEvent(new Event('change')));
  await advance(1000);
  expect(draw).toHaveBeenCalledTimes(count);
  expect(dispose).toHaveBeenCalledOnce();
});
it('pauses hidden/offscreen/zero-sized hosts without extending the deadline', async () => {
  const view = await playing();
  intersection(view.container, false);
  const count = draw.mock.calls.length;
  await advance(1000);
  expect(draw).toHaveBeenCalledTimes(count);
  intersection(view.container);
  await advance(100);
  expect(draw.mock.calls.length).toBeGreaterThan(count);
  const hidden = vi.spyOn(document, 'hidden', 'get').mockReturnValue(true);
  fireEvent(document, new Event('visibilitychange'));
  const after = draw.mock.calls.length;
  await advance(1000);
  expect(draw).toHaveBeenCalledTimes(after);
  hidden.mockReturnValue(false);
  fireEvent(document, new Event('visibilitychange'));
  await advance(100);
  expect(draw.mock.calls.length).toBeGreaterThan(after);
  vi.spyOn(HTMLElement.prototype, 'clientWidth', 'get').mockReturnValue(0);
  act(() => resize([], {} as ResizeObserver));
  const last = draw.mock.calls.length;
  await advance(3000);
  expect(draw).toHaveBeenCalledTimes(last);
  expect(dispose).toHaveBeenCalledOnce();
});
it('pauses a retained hidden ancestor and updates the palette without replay', async () => {
  const view = await playing();
  await act(async () => {
    view.container.hidden = true;
  });
  const count = draw.mock.calls.length;
  await advance(1000);
  expect(draw).toHaveBeenCalledTimes(count);
  await act(async () => {
    view.container.hidden = false;
    document.body.toggleAttribute('data-ds-dark-theme');
  });
  await advance(100);
  expect(draw.mock.calls.length).toBeGreaterThan(count);
  await advance(4000);
  expect(dispose).toHaveBeenCalledOnce();
  document.body.removeAttribute('data-ds-dark-theme');
});
it.each(['missing', 'throws'])('never imports when WebGL is %s', async (fault) => {
  if (fault === 'missing') context.mockReturnValue(null);
  else
    context.mockImplementation(() => {
      throw new Error('synthetic driver fault');
    });
  const view = render(<Backdrop placement="landing" />);
  intersection(view.container);
  await advance(100);
  expect(mocks.load).not.toHaveBeenCalled();
  expect(console.warn).toHaveBeenCalledExactlyOnceWith('Topographic decoration:', 'gl_unavailable');
});
it.each(['chunk', 'factory', 'draw', 'context'])(
  'contains a %s failure and never retries',
  async (fault) => {
    if (fault === 'chunk') mocks.load.mockResolvedValue(null);
    if (fault === 'factory')
      factory.mockImplementation(() => {
        throw new Error('synthetic allocation failure');
      });
    if (fault === 'draw')
      draw.mockImplementation(() => {
        throw new Error('synthetic draw failure');
      });
    const view = render(<Backdrop placement="landing" />);
    intersection(view.container);
    await advance(100);
    if (fault === 'context')
      fireEvent(view.container.querySelector('canvas')!, new Event('webglcontextlost'));
    fireEvent(view.container.querySelector('canvas')!, new Event('webglcontextrestored'));
    const calls = factory.mock.calls.length;
    await advance(5000);
    expect(factory).toHaveBeenCalledTimes(calls);
    expect(lose).toHaveBeenCalledOnce();
    expect(view.container.querySelector('canvas')).toHaveAttribute('hidden');
    expect(console.warn).toHaveBeenCalledTimes(1);
    expect(vi.getTimerCount()).toBe(0);
  },
);
it('disposes the capability context when an import arrives after unmount', async () => {
  let resolve!: (value: typeof factory) => void;
  mocks.load.mockReturnValue(
    new Promise((done) => {
      resolve = done;
    }),
  );
  const view = render(<Backdrop placement="landing" />);
  intersection(view.container);
  await advance(100);
  expect(mocks.load).toHaveBeenCalledOnce();
  view.unmount();
  await act(async () => resolve(factory));
  expect(factory).not.toHaveBeenCalled();
  expect(lose).toHaveBeenCalledOnce();
  expect(observers.every((item) => item.disconnect.mock.calls.length === 1)).toBe(true);
});
it('uses the original deadline after a visible route remount', async () => {
  const first = await playing();
  await advance(2000);
  first.unmount();
  const remount = render(<Backdrop placement="landing" />);
  intersection(remount.container);
  await advance(100);
  expect(factory).toHaveBeenCalledTimes(2);
  await advance(2000);
  expect(dispose).toHaveBeenCalledTimes(2);
  expect(vi.getTimerCount()).toBe(0);
});
it('does not allocate a renderer from a chunk that arrives while hidden', async () => {
  let resolve!: (value: typeof factory) => void;
  mocks.load.mockReturnValue(
    new Promise((done) => {
      resolve = done;
    }),
  );
  const view = render(<Backdrop placement="landing" />);
  intersection(view.container);
  await advance(100);
  intersection(view.container, false);
  await act(async () => resolve(factory));
  expect(factory).not.toHaveBeenCalled();
  intersection(view.container);
  await advance(100);
  expect(factory).toHaveBeenCalledOnce();
});
it('releases an expired pending import and ignores its later resolution', async () => {
  let resolve!: (value: typeof factory) => void;
  mocks.load.mockReturnValue(
    new Promise((done) => {
      resolve = done;
    }),
  );
  const view = render(<Backdrop placement="landing" />);
  intersection(view.container);
  await advance(4500);
  await act(async () => resolve(factory));
  expect(factory).not.toHaveBeenCalled();
  expect(lose).toHaveBeenCalledOnce();
  expect(vi.getTimerCount()).toBe(0);
});
it('releases every subscription even when renderer disposal throws', async () => {
  const view = await playing();
  dispose.mockImplementation(() => {
    throw new Error('synthetic disposal failure');
  });
  expect(() => view.unmount()).not.toThrow();
  expect(lose).toHaveBeenCalledOnce();
  expect(vi.getTimerCount()).toBe(0);
  expect(observers.every((item) => item.disconnect.mock.calls.length === 1)).toBe(true);
});
