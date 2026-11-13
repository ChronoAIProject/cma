import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it } from 'vitest';
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
