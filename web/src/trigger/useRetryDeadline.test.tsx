import { act, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { useRetryDeadline } from './useRetryDeadline';

function RetryControl({ deadline }: { readonly deadline: number | null }) {
  const blocked = useRetryDeadline(deadline);
  return <button disabled={blocked}>Retry</button>;
}

afterEach(() => {
  vi.useRealTimers();
});

describe('useRetryDeadline', () => {
  it('blocks until the exact deadline and blocks again for a replacement deadline', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-08-28T00:00:00Z'));
    const firstDeadline = Date.now() + 10_000;
    const view = render(<RetryControl deadline={firstDeadline} />);

    expect(screen.getByRole('button', { name: 'Retry' })).toBeDisabled();
    await act(async () => vi.advanceTimersByTimeAsync(9_999));
    expect(screen.getByRole('button', { name: 'Retry' })).toBeDisabled();
    await act(async () => vi.advanceTimersByTimeAsync(1));
    expect(screen.getByRole('button', { name: 'Retry' })).toBeEnabled();

    const replacementDeadline = Date.now() + 5_000;
    view.rerender(<RetryControl deadline={replacementDeadline} />);
    expect(screen.getByRole('button', { name: 'Retry' })).toBeDisabled();
    await act(async () => vi.advanceTimersByTimeAsync(5_000));
    expect(screen.getByRole('button', { name: 'Retry' })).toBeEnabled();
  });

  it('does not block when no deadline is present', () => {
    render(<RetryControl deadline={null} />);
    expect(screen.getByRole('button', { name: 'Retry' })).toBeEnabled();
  });
});
