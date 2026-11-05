import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@chronoai/cma-sdk';
import { UsageDialog } from './UsageDialog';
import { metricsClient, reportFixture } from './metrics/testFixtures';

vi.mock('./metrics/MetricChart', () => ({ MetricChart: () => <p>Measured chart</p> }));
afterEach(() => window.history.replaceState(null, '', '/'));
it('defers Agent discovery until an owner report authorizes the selected Sandbox', async () => {
  window.history.replaceState(null, '', '/usage?sandbox=sbx-1');
  const actor = userEvent.setup();
  const api = metricsClient(false);
  const report = reportFixture(false);
  api.create.mockResolvedValue({
    ...report,
    query: { ...report.query, filters: { ...report.query.filters, sandbox_id: 'sbx-1' } },
  });
  const agents = {
    list: vi.fn().mockResolvedValue({
      items: [{ id: 'agt-1', title: 'Research Agent' }],
      next: 'agents-next',
    }),
  };
  const client = {
    readMeasuredUsage: vi.fn().mockResolvedValue(false),
    usageMetrics: api,
    usage: { events: vi.fn() },
    agents,
  };
  render(<UsageDialog client={client} open onClose={vi.fn()} />);
  await screen.findByRole('button', { name: 'Agent: All Agents in this Sandbox' });
  expect(agents.list).not.toHaveBeenCalled();
  await actor.click(screen.getByRole('button', { name: 'Agent: All Agents in this Sandbox' }));
  await waitFor(() =>
    expect(agents.list).toHaveBeenCalledWith(
      expect.objectContaining({ sandbox: 'sbx-1', limit: 50 }),
    ),
  );
  await actor.click(screen.getByRole('button', { name: 'Next Agents' }));
  await waitFor(() =>
    expect(agents.list).toHaveBeenLastCalledWith(
      expect.objectContaining({ sandbox: 'sbx-1', cursor: 'agents-next', limit: 50 }),
    ),
  );
  await actor.click(screen.getByRole('button', { name: 'Usage Agent' }));
  await actor.click(screen.getByRole('menuitemradio', { name: 'Research Agent' }));
  await actor.click(screen.getByRole('button', { name: 'Apply usage filters' }));
  await waitFor(() =>
    expect(api.create).toHaveBeenLastCalledWith(
      expect.objectContaining({
        filters: expect.objectContaining({ sandbox_id: 'sbx-1', agent_id: 'agt-1' }),
      }),
      expect.anything(),
    ),
  );
  expect(screen.getByText(/Shared Sandbox usage: Agent selection resolves/)).toBeInTheDocument();
});
it('never discovers Agents when the owner Sandbox report is refused', async () => {
  window.history.replaceState(null, '', '/usage?sandbox=shared-sandbox&owner=someone-else');
  const api = metricsClient(false);
  api.create.mockRejectedValue(new ApiError(404, 'Not owned'));
  const agents = { list: vi.fn() };
  render(
    <UsageDialog
      client={{
        readMeasuredUsage: vi.fn().mockResolvedValue(false),
        usageMetrics: api,
        usage: { events: vi.fn() },
        agents,
      }}
      open
      onClose={vi.fn()}
    />,
  );
  await screen.findByText('Usage access unavailable');
  expect(agents.list).not.toHaveBeenCalled();
  expect(screen.queryByRole('button', { name: /Agent: / })).not.toBeInTheDocument();
  expect(api.create.mock.calls[0]?.[0].filters?.owner).toBeUndefined();
});
it('keeps the popup usable when the selected client has no usage adapter', () => {
  render(<UsageDialog open onClose={vi.fn()} />);
  expect(screen.getByRole('dialog', { name: 'Your usage' })).toBeInTheDocument();
  expect(screen.getByRole('status')).toHaveTextContent('Usage reporting is unavailable');
  expect(screen.getByRole('button', { name: 'Close' })).toBeEnabled();
});
