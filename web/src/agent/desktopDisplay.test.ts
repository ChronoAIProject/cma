import { afterEach, expect, it, vi } from 'vitest';
import { watchDesktopDisplay } from './desktopDisplay';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

function fixture() {
  vi.useFakeTimers();
  const target = document.createElement('div');
  const canvas = document.createElement('canvas');
  target.append(canvas);
  const bounds = { width: 800, height: 500 };
  const size = { width: 800, height: 500 };
  vi.spyOn(target, 'getBoundingClientRect').mockImplementation(() => bounds as DOMRect);
  vi.spyOn(canvas, 'getBoundingClientRect').mockImplementation(() => size as DOMRect);
  const pixel = new Uint8ClampedArray([0, 0, 0, 255]);
  const read = vi.fn(() => ({ data: pixel }) as ImageData);
  vi.spyOn(canvas, 'getContext').mockReturnValue({
    getImageData: read,
  } as unknown as CanvasRenderingContext2D);
  const lost = vi.fn();
  const rescale = vi.fn();
  const stop = watchDesktopDisplay(target, rescale, lost);
  return { target, canvas, bounds, size, pixel, read, lost, rescale, stop };
}

it('accepts an idle opaque black desktop and ignores one transient repaint', () => {
  const f = fixture();
  vi.advanceTimersByTime(10_000);
  expect(f.read).toHaveBeenCalledTimes(5);
  expect(f.lost).not.toHaveBeenCalled();
  f.pixel[3] = 0;
  vi.advanceTimersByTime(2_000);
  f.pixel[3] = 255;
  vi.advanceTimersByTime(2_000);
  expect(f.lost).not.toHaveBeenCalled();
  f.stop();
  expect(vi.getTimerCount()).toBe(0);
});

it.each(['transparent', 'missing', 'unreadable', 'zero-size'])(
  'reports a persistently %s display once and stops observation',
  (mode) => {
    const f = fixture();
    if (mode === 'transparent') f.pixel[3] = 0;
    if (mode === 'missing') f.canvas.remove();
    if (mode === 'zero-size') f.size.width = 0;
    if (mode === 'unreadable')
      f.read.mockImplementation(() => {
        throw new DOMException('Context unavailable');
      });
    vi.advanceTimersByTime(2_000);
    expect(f.lost).not.toHaveBeenCalled();
    vi.advanceTimersByTime(2_000);
    expect(f.lost).toHaveBeenCalledOnce();
    expect(vi.getTimerCount()).toBe(0);
    f.canvas.dispatchEvent(new Event('contextlost'));
    expect(f.lost).toHaveBeenCalledOnce();
  },
);

it('rescales a collapsed canvas without reconnecting if its pixels survive', () => {
  const f = fixture();
  f.size.width = 0;
  f.rescale.mockImplementation(() => {
    f.size.width = 800;
  });
  vi.advanceTimersByTime(4_000);
  expect(f.rescale).toHaveBeenCalledOnce();
  expect(f.lost).not.toHaveBeenCalled();
  f.stop();
});

it('does not read or recover a hidden pane and resumes checking when visible', () => {
  const f = fixture();
  f.pixel[3] = 0;
  f.bounds.width = 0;
  vi.advanceTimersByTime(10_000);
  expect(f.read).not.toHaveBeenCalled();
  expect(f.lost).not.toHaveBeenCalled();
  f.bounds.width = 800;
  const hidden = vi.spyOn(document, 'hidden', 'get').mockReturnValue(true);
  vi.advanceTimersByTime(10_000);
  expect(f.read).not.toHaveBeenCalled();
  hidden.mockReturnValue(false);
  document.dispatchEvent(new Event('visibilitychange'));
  expect(f.rescale).toHaveBeenCalledOnce();
  vi.advanceTimersByTime(2_000);
  expect(f.lost).toHaveBeenCalledOnce();
});

it('handles non-bubbling context loss immediately and removes its listeners on stop', () => {
  const f = fixture();
  f.canvas.dispatchEvent(new Event('contextlost'));
  expect(f.lost).toHaveBeenCalledOnce();
  document.dispatchEvent(new Event('visibilitychange'));
  expect(f.rescale).not.toHaveBeenCalled();
  expect(vi.getTimerCount()).toBe(0);
  const next = fixture();
  next.stop();
  next.canvas.dispatchEvent(new Event('contextlost'));
  expect(next.lost).not.toHaveBeenCalled();
});
