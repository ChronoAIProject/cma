import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { MetricsDashboard } from './MetricsDashboard';
import { defaultSelection } from './model';
import { metricsClient } from './testFixtures';
import { useMeasuredUsage, usageSettingsChanged, type ReadMeasuredUsage } from './useMeasuredUsage';

vi.mock('./MetricChart', () => ({ MetricChart: () => <p>Measured chart</p> }));
it('reflects saved visibility and fails closed without dropping allocated detail', async () => {
  const api = metricsClient();
  const history = { events: vi.fn() };
  const read = vi.fn().mockResolvedValue(false);
  function Usage({ readSetting }: { readSetting: ReadMeasuredUsage }) {
    const measured = useMeasuredUsage(readSetting);
    return (
      <MetricsDashboard
        api={api}
        history={history}
        admin={false}
        measured={measured}
        selection={defaultSelection}
        onSelection={vi.fn()}
      />
    );
  }
  const actor = userEvent.setup();
  render(<Usage readSetting={read} />);
  await screen.findByRole('region', { name: 'Allocated capacity now' });
  expect(api.series).not.toHaveBeenCalled();
  read.mockResolvedValue(true);
  act(usageSettingsChanged);
  await screen.findByRole('region', { name: 'Measured resource charts' });
  await screen.findByText('Measured chart');
  await actor.click(screen.getByRole('button', { name: 'Research Sandbox' }));
  const detail = screen.getByRole('region', { name: 'Sandbox usage details' });
  await waitFor(() => expect(within(detail).getAllByText('Measured chart')).toHaveLength(3));
  const previousSeries = api.series.mock.calls.length;
  read.mockRejectedValue(new Error('Settings unavailable'));
  act(usageSettingsChanged);
  await within(detail).findByRole('region', { name: 'Allocated capacity now' });
  expect(screen.queryByText('Measured chart')).not.toBeInTheDocument();
  expect(api.series).toHaveBeenCalledTimes(previousSeries);
  expect(
    within(detail).getByRole('region', { name: 'Allocated resource-hours' }),
  ).toBeInTheDocument();
});
