import { act, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { Composer } from './Composer';

afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

it('coalesces width changes outside ResizeObserver delivery and cancels work on unmount', async () => {
  vi.useFakeTimers();
  let changed!: () => void;
  vi.stubGlobal(
    'ResizeObserver',
    class {
      constructor(callback: () => void) {
        changed = callback;
      }
      observe() {}
      disconnect() {}
    },
  );
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
  let width = 200;
  let height = 120;
  Object.defineProperty(field, 'clientWidth', { get: () => width });
  Object.defineProperty(field, 'scrollHeight', { get: () => height });
  const before = field.style.height;
  act(() => {
    changed();
    width = 300;
    changed();
  });
  expect(field.style.height).toBe(before);
  await act(() => vi.advanceTimersToNextFrame());
  expect(field.style.height).not.toBe(before);
  const fitted = field.style.height;
  act(() => {
    width = 400;
    height = 240;
    changed();
  });
  view.unmount();
  await act(() => vi.advanceTimersToNextFrame());
  expect(field.style.height).toBe(fitted);
});
