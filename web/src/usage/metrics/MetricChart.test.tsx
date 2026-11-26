import { act, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { MetricChart } from './MetricChart';
import { seriesFixture } from './testFixtures';

it('exposes accessible chart buckets, null gaps, units and ratio of matching totals', async () => {
  const actor = userEvent.setup();
  render(<MetricChart series={seriesFixture('cpu')} capacity={2} />);
  await actor.click(screen.getByText('View CPU chart data'));
  const table = await screen.findByRole('table', { name: 'CPU chart data' });
  expect(within(table).getByRole('cell', { name: '0.5 cores' })).toBeInTheDocument();
  expect(within(table).getAllByRole('cell', { name: 'Unavailable' }).length).toBeGreaterThan(0);
  expect(within(table).getByText('25%')).toBeInTheDocument();
  expect(within(table).getByText(/no measurement/)).toBeInTheDocument();
  expect(screen.getByText(/allocated now 2 cores/)).toBeInTheDocument();
});
it('presents empty cohorts and missing samples distinctly', () => {
  const series = seriesFixture('disk');
  const { rerender } = render(<MetricChart series={{ ...series, lines: [] }} />);
  expect(screen.getByRole('status')).toHaveTextContent('No Sandboxes in this selection.');
  rerender(
    <MetricChart
      series={{
        ...series,
        lines: series.lines.map((line) => ({
          ...line,
          points: line.points.map((point) => ({ ...point, mean: null })),
        })),
      }}
    />,
  );
  expect(screen.getByRole('status')).toHaveTextContent('No measured samples in this range.');
});

it('waits for both dimensions and restores the chart when its dialog reopens', () => {
  let width = 0;
  let height = 0;
  let notify: () => void = () => {};
  const disconnect = vi.fn();
  const getWidth = vi
    .spyOn(HTMLElement.prototype, 'clientWidth', 'get')
    .mockImplementation(() => width);
  const getHeight = vi
    .spyOn(HTMLElement.prototype, 'clientHeight', 'get')
    .mockImplementation(() => height);
  vi.stubGlobal(
    'ResizeObserver',
    class {
      constructor(private readonly callback: ResizeObserverCallback) {}
      observe(target: Element) {
        if (target.classList.contains('usage-metric-chart__plot')) {
          notify = () => this.callback([], this);
        }
      }
      unobserve() {}
      disconnect = disconnect;
    },
  );
  try {
    const { container, unmount } = render(<MetricChart series={seriesFixture('cpu')} />);
    const plot = container.querySelector('.usage-metric-chart__plot')!;
    expect(plot.querySelector('svg')).toBeNull();

    width = 480;
    act(notify);
    expect(plot.querySelector('svg')).toBeNull();
    height = 240;
    act(notify);
    expect(plot.querySelector('svg')).toHaveAttribute('width', '480');

    width = 0;
    height = 0;
    act(notify);
    expect(plot.querySelector('svg')).toBeNull();

    width = 320;
    height = 240;
    act(notify);
    expect(plot.querySelector('svg')).toHaveAttribute('width', '320');
    unmount();
    expect(disconnect).toHaveBeenCalled();
  } finally {
    vi.unstubAllGlobals();
    getWidth.mockRestore();
    getHeight.mockRestore();
  }
});
