import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { expect, it, vi } from 'vitest';
import type { UsageFilterOptionsQuery, UsageMetricQuery } from '@cma/client';
import { OpsMetricFilters } from './OpsMetricFilters';
import { MetricsDashboard } from './MetricsDashboard';
import { defaultSelection } from './model';
import { metricsClient, reportFixture } from './testFixtures';

it('offers only dropdown filters, preserves Custom validation and clears to defaults', async () => {
  const apply = vi.fn();
  render(
    <OpsMetricFilters
      api={metricsClient(true)}
      selection={defaultSelection}
      active
      onApply={apply}
    />,
  );
  expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
  expect(screen.queryByRole('searchbox')).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: 'Usage time range' }));
  await userEvent.click(screen.getByRole('menuitemradio', { name: 'Custom (up to 30 days)' }));
  fireEvent.change(screen.getByLabelText('Usage range from'), {
    target: { value: '2020-01-01T00:00' },
  });
  fireEvent.change(screen.getByLabelText('Usage range to'), {
    target: { value: '2020-03-01T00:00' },
  });
  await userEvent.click(screen.getByRole('button', { name: 'Apply usage filters' }));
  await screen.findByRole('alert');
  expect(apply).not.toHaveBeenCalled();
  fireEvent.change(screen.getByLabelText('Usage range to'), {
    target: { value: '2020-01-02T00:00' },
  });
  await userEvent.click(screen.getByRole('button', { name: 'Apply usage filters' }));
  expect(apply).toHaveBeenCalledWith(expect.objectContaining({ range: 'custom' }));
  await userEvent.click(screen.getByRole('button', { name: 'Clear filters' }));
  expect(apply).toHaveBeenLastCalledWith(defaultSelection);
});
it('narrows Sandbox discovery by the draft owner and applies one report while the prior report is pending', async () => {
  const api = metricsClient(true);
  api.create.mockImplementation(async (input: UsageMetricQuery) => ({
    ...reportFixture(true),
    query: {
      ...reportFixture(true).query,
      filters: { ...reportFixture(true).query.filters, ...input.filters },
    },
    status: 'pending',
    summary: null,
  }));
  api.options.mockImplementation(async (query: UsageFilterOptionsQuery) => ({
    items:
      query.kind === 'owner'
        ? [{ value: 'owner-two', label: 'Morgan · account reference' }]
        : [{ value: 'sandbox-two', label: 'Research' }],
    next: null,
  }));
  function Dashboard() {
    const [selection, update] = useState(defaultSelection);
    return (
      <MetricsDashboard
        api={api}
        history={{ events: vi.fn() }}
        admin
        selection={selection}
        onSelection={update}
      />
    );
  }
  render(<Dashboard />);
  await screen.findByText('Preparing usage report…');
  await userEvent.click(screen.getByRole('button', { name: 'Owner: Everyone' }));
  await userEvent.click(
    await screen.findByRole('menuitemradio', { name: 'Morgan · account reference' }),
  );
  await userEvent.click(screen.getByRole('button', { name: 'Sandbox: All Sandboxes' }));
  await screen.findByRole('menuitemradio', { name: 'Research' });
  expect(api.options).toHaveBeenLastCalledWith(
    expect.objectContaining({ kind: 'sandbox', owner: 'owner-two' }),
  );
  await userEvent.click(screen.getByRole('menuitemradio', { name: 'Research' }));
  expect(api.create).toHaveBeenCalledTimes(1);
  await userEvent.click(screen.getByRole('button', { name: 'Apply usage filters' }));
  await waitFor(() => expect(api.create).toHaveBeenCalledTimes(2));
  expect(api.create).toHaveBeenLastCalledWith(
    expect.objectContaining({
      filters: expect.objectContaining({
        owner: 'owner-two',
        sandbox_id: 'sandbox-two',
        search: '',
      }),
    }),
    expect.anything(),
  );
  expect(api.report).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button', { name: 'Clear filters' }));
  // The exact default key retained its pending admission while the owner and
  // Sandbox selection was active, so clearing filters resumes that report
  // instead of creating a duplicate admission.
  await waitFor(() => expect(api.report).toHaveBeenCalledWith('report-one', expect.anything()), {
    timeout: 2000,
  });
  expect(api.create).toHaveBeenCalledTimes(2);
  expect(api.create.mock.calls[0]![0].filters).not.toHaveProperty('owner');
});

it.each([' Live', 'Live '])('finds a status with surrounding whitespace %j', async (search) => {
  const actor = userEvent.setup();
  const apply = vi.fn();
  render(
    <OpsMetricFilters
      api={metricsClient(true)}
      selection={defaultSelection}
      active
      onApply={apply}
    />,
  );
  await actor.click(screen.getByRole('button', { name: 'Usage Sandbox status' }));
  await actor.type(screen.getByRole('textbox', { name: 'Search usage sandbox status' }), search);
  await actor.click(screen.getByRole('menuitemradio', { name: 'Live' }));
  await actor.click(screen.getByRole('button', { name: 'Apply usage filters' }));
  expect(apply).toHaveBeenCalledWith(expect.objectContaining({ status: 'active' }));
});
