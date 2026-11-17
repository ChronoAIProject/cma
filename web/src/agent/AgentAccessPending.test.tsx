import { act, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@cma/client';
import { AgentAccessPending } from './AgentAccessPending';
import { useAuthorizedRead } from '../resources/useAuthorizedRead';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});
it('shows a busy countdown and automatically opens after a detail 429', async () => {
  vi.useFakeTimers();
  vi.spyOn(Math, 'random').mockReturnValue(0);
  const read = vi
    .fn()
    .mockRejectedValueOnce(new ApiError(429, 'Caller exhausted', undefined, undefined, 5000))
    .mockResolvedValue({ value: 'Agent ready' });
  function Page() {
    const { state, reload } = useAuthorizedRead<string>(read);
    return state.status === 'ready' ? (
      <p>{state.value}</p>
    ) : (
      <AgentAccessPending state={state} onRetry={reload} />
    );
  }
  const page = render(<Page />);
  await act(async () => {});
  expect(screen.queryByText('Agent unavailable')).not.toBeInTheDocument();
  expect(screen.getByText('Busy, retrying in 5s…')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Check access again' })).toBeDisabled();
  await act(() => vi.advanceTimersByTimeAsync(2000));
  expect(screen.getByText('Busy, retrying in 3s…')).toBeInTheDocument();
  await act(() => vi.advanceTimersByTimeAsync(3000));
  expect(screen.getByText('Agent ready')).toBeInTheDocument();
  expect(read).toHaveBeenCalledTimes(2);
  page.unmount();
});
it('keeps a confirmed refusal visible as an access error', () => {
  render(
    <AgentAccessPending state={{ status: 'error', message: 'Forbidden' }} onRetry={vi.fn()} />,
  );
  expect(screen.getByRole('heading', { name: 'Agent unavailable' })).toBeInTheDocument();
  expect(screen.getByText('Forbidden')).toBeInTheDocument();
});
