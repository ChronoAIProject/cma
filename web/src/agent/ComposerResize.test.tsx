import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { Composer } from './Composer';

afterEach(() => vi.unstubAllGlobals());

it('fits width changes outside resize delivery and cancels pending work on unmount', () => {
  const observations: {
    callback: ResizeObserverCallback;
    target?: Element;
    disconnect: () => void;
  }[] = [];
  vi.stubGlobal(
    'ResizeObserver',
    class {
      readonly entry;
      constructor(callback: ResizeObserverCallback) {
        this.entry = { callback, target: undefined as Element | undefined, disconnect: vi.fn() };
        observations.push(this.entry);
      }
      observe(target: Element) {
        this.entry.target = target;
      }
      disconnect() {
        this.entry.disconnect();
      }
    },
  );
  let frameId = 0;
  const frames = new Map<number, FrameRequestCallback>();
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => {
    frames.set(++frameId, callback);
    return frameId;
  });
  vi.stubGlobal('cancelAnimationFrame', (id: number) => frames.delete(id));
  const view = render(
    <Composer
      status="ready"
      running={false}
      busy={false}
      inputAvailability={{ allowed: true }}
      onSend={vi.fn()}
      onStop={vi.fn()}
    />,
  );
  const field = screen.getByRole('textbox', { name: 'Message' });
  const observation = observations.find((entry) => entry.target === field)!;
  Object.defineProperty(field, 'scrollHeight', { configurable: true, value: 24 });
  fireEvent.change(field, { target: { value: 'Keep this unsent draft' } });
  expect(field.style.height).toBe('24px');
  Object.defineProperty(field, 'scrollHeight', { configurable: true, value: 96 });
  const resized = (width: number) => {
    Object.defineProperty(field, 'clientWidth', { configurable: true, value: width });
    act(() => observation.callback([], {} as ResizeObserver));
  };
  resized(240);
  resized(180);
  expect(field.style.height).toBe('24px');
  expect(frames.size).toBe(1);
  act(() => {
    const pending = [...frames.values()];
    frames.clear();
    pending.forEach((callback) => callback(0));
  });
  expect(field.style.height).toBe('96px');
  expect(field).toHaveValue('Keep this unsent draft');
  resized(180);
  expect(frames.size).toBe(0);
  resized(300);
  expect(frames.size).toBe(1);
  view.unmount();
  expect(observation.disconnect).toHaveBeenCalledOnce();
  expect(frames.size).toBe(0);
});
