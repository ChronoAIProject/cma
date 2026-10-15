import { act, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { TurnUsage } from './TurnUsage';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it('animates towards actual reported usage and stops there until another report arrives', async () => {
  vi.useFakeTimers();
  const view = render(<TurnUsage usage={{ inputTokens: 50, outputTokens: 1000 }} />);
  const output = screen.getByLabelText('Turn output tokens: 1,000');
  expect(output).toHaveTextContent('0 output');
  await act(() => vi.advanceTimersByTimeAsync(200));
  const midway = Number(output.textContent?.replaceAll(',', '').split(' ')[0]);
  expect(midway).toBeGreaterThan(0);
  expect(midway).toBeLessThan(1000);
  await act(() => vi.advanceTimersByTimeAsync(500));
  expect(output).toHaveTextContent('1,000 output');
  await act(() => vi.advanceTimersByTimeAsync(2000));
  expect(output).toHaveTextContent('1,000 output');
  view.rerender(<TurnUsage usage={{ inputTokens: 70, outputTokens: 4000 }} />);
  expect(screen.getByLabelText('Turn output tokens: 4,000')).toHaveTextContent('1,000 output');
  await act(() => vi.advanceTimersByTimeAsync(700));
  expect(output).toHaveTextContent('4,000 output');
});

it('shows unreported usage explicitly and immediately settles motion for reduced-motion users', async () => {
  vi.useFakeTimers();
  const original = window.matchMedia;
  vi.spyOn(window, 'matchMedia').mockImplementation((query) => ({
    ...original(query),
    matches: true,
  }));
  const view = render(<TurnUsage />);
  expect(screen.getByText('Turn tokens · waiting')).toBeInTheDocument();
  view.rerender(<TurnUsage usage={{ inputTokens: 0, outputTokens: 1000 }} />);
  await act(() => vi.advanceTimersByTimeAsync(20));
  expect(screen.getByLabelText('Turn output tokens: 1,000')).toHaveTextContent('1,000 output');
});
