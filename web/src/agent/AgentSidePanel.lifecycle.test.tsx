import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@cma/client';
import { AGENT_DETAIL } from '../test/agent';
import { computerFixture, live, paused } from './computerLifecycle.fixture';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it.each(['files', 'browser'] as const)(
  'holds %s probes during a confirmed pause without retries or writes',
  async (panel) => {
    vi.useFakeTimers();
    const f = computerFixture();
    f.files.environment.mockRejectedValue(new ApiError(503, 'Not a pause classification'));
    render(f.draw({ panel, detail: { ...AGENT_DETAIL, desktop: true } }));
    await act(() => vi.advanceTimersByTimeAsync(0));
    const tab = within(screen.getByRole('tabpanel'));
    expect(tab.getByRole('status')).toHaveTextContent('Sandbox paused. Your work is preserved.');
    expect(tab.queryByRole('button', { name: /Try again|Check environment again/ })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Reconnect' })).toBeNull();
    for (let i = 0; i < 3; i++) {
      fireEvent.focus(window);
      await act(() => vi.advanceTimersByTimeAsync(30_000));
    }
    expect(f.files.environment).not.toHaveBeenCalled();
    expect(f.files.list).not.toHaveBeenCalled();
    expect(f.browser.read).not.toHaveBeenCalled();
    expect(f.browser.configure).not.toHaveBeenCalled();
    expect(f.files.write).not.toHaveBeenCalled();
    for (const mutation of Object.values(f.mutations)) expect(mutation).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Close computer' }));
    expect(f.onClose).toHaveBeenCalledOnce();
  },
);

it('keeps independently authorized recorded operations readable while paused without environment IO', async () => {
  const f = computerFixture();
  render(f.draw({ panel: 'activity' }));
  await act(async () => {});
  expect(screen.getByText('retained-output', { exact: true })).toBeVisible();
  expect(f.files.environment).not.toHaveBeenCalled();
});

it.each([
  ['checking', 'Checking Sandbox resources.'],
  ['unavailable', 'Sandbox status is unavailable. Live resources are held.'],
  ['inactive', 'Pausing. Agent conversations and history remain available.'],
] as const)('uses current %s authority instead of a stale paused detail', async (state, notice) => {
  const f = computerFixture();
  render(f.draw({ resourceAvailability: { state, notice } }));
  await act(async () => {});
  const tab = within(screen.getByRole('tabpanel'));
  expect(tab.getByRole('status')).toHaveTextContent(notice);
  expect(tab.queryByText(/Sandbox paused/)).toBeNull();
  expect(f.files.environment).not.toHaveBeenCalled();
});

it('keeps a true live environment 503 retryable despite stale paused detail', async () => {
  const f = computerFixture();
  f.files.environment.mockRejectedValueOnce(new ApiError(503, 'Private upstream error'));
  render(f.draw({ resourceAvailability: live }));
  await act(async () => {});
  const tab = within(screen.getByRole('tabpanel'));
  expect(f.files.list).not.toHaveBeenCalled();
  expect(tab.getByText('We couldn’t check file availability.')).toBeVisible();
  expect(tab.queryByText(/Sandbox paused/)).toBeNull();
  fireEvent.focus(window);
  await act(async () => {});
  expect(f.files.environment).toHaveBeenCalledTimes(2);
  expect(f.files.list).toHaveBeenCalledTimes(1);
});

it('requires separate shared history permission during a pause', async () => {
  const f = computerFixture();
  render(f.draw({ shared: true, panel: 'activity', resourceAvailability: paused }));
  await act(async () => {});
  expect(screen.queryByText('retained-output', { exact: true })).toBeNull();
  expect(f.files.environment).not.toHaveBeenCalled();
});
