import { act, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { Menu } from './Menu';

afterEach(() => vi.unstubAllGlobals());

it('defers resize placement and cancels it when the menu closes', () => {
  let resized: ResizeObserverCallback;
  const disconnect = vi.fn();
  vi.stubGlobal(
    'ResizeObserver',
    class {
      constructor(callback: ResizeObserverCallback) {
        resized = callback;
      }
      observe() {}
      disconnect = disconnect;
    },
  );
  let frameId = 0;
  const frames = new Map<number, FrameRequestCallback>();
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => {
    frames.set(++frameId, callback);
    return frameId;
  });
  vi.stubGlobal('cancelAnimationFrame', (id: number) => frames.delete(id));
  const props = {
    anchor: <button>Choices</button>,
    items: [{ id: 'all', label: 'Everyone' }],
    matchAnchorWidth: true,
    onSelect: vi.fn(),
    onClose: vi.fn(),
  };
  const view = render(<Menu {...props} open />);
  const menu = screen.getByRole('menu');
  const anchor = screen.getByRole('button', { name: 'Choices' }).parentElement!;
  let width = 160;
  anchor.getBoundingClientRect = () => DOMRect.fromRect({ x: 200, y: 100, width, height: 44 });
  expect(menu.style.minWidth).toBe('0px');
  act(() => resized([], {} as ResizeObserver));
  width = 240;
  act(() => resized([], {} as ResizeObserver));
  expect(menu.style.minWidth).toBe('0px');
  expect(frames.size).toBe(1);
  act(() => {
    const pending = [...frames.values()];
    frames.clear();
    pending.forEach((callback) => callback(0));
  });
  expect(menu.style.minWidth).toBe('240px');
  expect(menu.style.visibility).toBe('visible');
  expect(screen.getByRole('menuitem', { name: 'Everyone' })).toHaveFocus();
  act(() => resized([], {} as ResizeObserver));
  expect(frames.size).toBe(1);
  view.rerender(<Menu {...props} open={false} />);
  expect(disconnect).toHaveBeenCalledOnce();
  expect(frames.size).toBe(0);
  expect(screen.getByRole('button', { name: 'Choices' })).toHaveFocus();
  view.unmount();
});
