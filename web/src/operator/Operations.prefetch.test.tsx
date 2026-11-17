import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { Operations } from './Operations';
import { operatorClient } from './testFixtures';
import { reportFixture } from '../usage/metrics/testFixtures';

const flush = () => act(async () => {});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  window.history.replaceState(null, '', '/');
});

it.each([false, true])(
  'releases default polling when the dashboard changes filters (prefetched=%s)',
  async (prefetched) => {
    vi.useFakeTimers();
    vi.spyOn(Math, 'random').mockReturnValue(0);
    window.history.replaceState(null, '', prefetched ? '/operations?tab=activity' : '/operations');
    const client = operatorClient();
    client.adminUsageMetrics.create.mockResolvedValue({
      ...reportFixture(true),
      status: 'pending',
      summary: null,
      retry_after_seconds: 2,
    });
    const view = render(
      <Operations client={client} capabilities={['read_all_sandboxes', 'read_activity_log']} />,
    );
    await flush();
    expect(client.adminUsageMetrics.create).toHaveBeenCalledTimes(1);
    if (prefetched) {
      fireEvent.click(screen.getByRole('tab', { name: 'Sandboxes & usage' }));
      await flush();
      expect(client.adminUsageMetrics.create).toHaveBeenCalledTimes(1);
    }
    // The completed admission is retained; only the selected job may be polled.
    fireEvent.click(screen.getByRole('button', { name: 'Usage time range' }));
    fireEvent.click(screen.getByRole('menuitemradio', { name: 'Last 7 days' }));
    fireEvent.click(screen.getByRole('button', { name: 'Apply usage filters' }));
    await flush();
    expect(client.adminUsageMetrics.create).toHaveBeenCalledTimes(2);
    const tab = screen.getByRole('tab', { name: 'Sandboxes & usage' });
    fireEvent.mouseEnter(tab);
    fireEvent.focus(tab);
    await flush();
    expect(client.adminUsageMetrics.create).toHaveBeenCalledTimes(2);
    await act(async () => vi.advanceTimersByTimeAsync(2000));
    expect(client.adminUsageMetrics.report).toHaveBeenCalledTimes(1);
    view.unmount();
    await flush();
  },
);

it('adopts an in-flight prefetch when the usage dashboard opens', async () => {
  window.history.replaceState(null, '', '/operations?tab=activity');
  const client = operatorClient();
  let resolve!: (report: ReturnType<typeof reportFixture>) => void;
  client.adminUsageMetrics.create.mockReturnValue(
    new Promise((done) => {
      resolve = done;
    }),
  );
  const view = render(
    <Operations client={client} capabilities={['read_all_sandboxes', 'read_activity_log']} />,
  );
  await flush();
  const signal = client.adminUsageMetrics.create.mock.calls[0]![1].signal as AbortSignal;
  fireEvent.click(screen.getByRole('tab', { name: 'Sandboxes & usage' }));
  await flush();
  expect(signal.aborted).toBe(false);
  expect(client.adminUsageMetrics.create).toHaveBeenCalledTimes(1);
  await act(async () => resolve(reportFixture(true)));
  expect(view.container.querySelector('.usage-report')).toBeInTheDocument();
  view.unmount();
  await flush();
});
