import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError, type UsageMetricQuery } from '@cma/client';
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
    read: vi.fn(),
    list: vi.fn().mockResolvedValue({
      items: [{ id: 'agt-1', sandboxId: 'sbx-1', name: 'Research Agent' }],
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
  expect(agents.read).not.toHaveBeenCalled();
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
  await actor.click(screen.getByRole('menuitemradio', { name: 'Research Agent (agt-1)' }));
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

const principal = { principal: 'owner-1', cacheKey: 'owner-1/epoch-1' };
function snapshot(query: UsageMetricQuery, id: string, subject = 'owner-1') {
  const base = reportFixture(false, id);
  return {
    ...base,
    scope: { principal: subject, admin: false },
    query: {
      ...base.query,
      ...query,
      filters: {
        ...base.query.filters,
        ...Object.fromEntries(
          Object.entries(query.filters ?? {}).filter(([, value]) => value !== undefined),
        ),
      },
    },
  };
}
function clientFixture() {
  const usageMetrics = metricsClient();
  let sequence = 0;
  usageMetrics.create.mockImplementation(async (query: UsageMetricQuery) =>
    snapshot(query, `report-${++sequence}`),
  );
  return {
    readMeasuredUsage: vi.fn().mockResolvedValue(false),
    usageMetrics,
    usage: { events: vi.fn() },
    agents: {
      read: vi.fn().mockResolvedValue({ id: 'agt-1', sandboxId: 'sbx-1', name: 'Research Agent' }),
      list: vi.fn().mockResolvedValue({
        items: [{ id: 'agt-1', sandboxId: 'sbx-1', name: 'Research Agent' }],
        next: null,
      }),
    },
  };
}

it('preserves both chosen names through Apply, Agent-picker unmount, and dialog close/reopen', async () => {
  const client = clientFixture();
  const props = { client, principal, onClose: vi.fn() };
  const { rerender } = render(<UsageDialog {...props} open />);
  await screen.findByRole('button', { name: 'Research Sandbox' });
  await userEvent.click(screen.getByRole('button', { name: 'Sandbox: All my Sandboxes' }));
  await userEvent.click(await screen.findByRole('button', { name: 'Choose sandbox' }));
  await userEvent.click(screen.getByRole('menuitemradio', { name: 'Research Sandbox (sbx-1)' }));
  expect(window.location.search).toBe('');
  expect(screen.getByRole('button', { name: 'Sandbox: Research Sandbox' })).toBeVisible();
  await userEvent.click(screen.getByRole('button', { name: 'Apply usage filters' }));
  await userEvent.click(
    await screen.findByRole('button', { name: 'Agent: All Agents in this Sandbox' }),
  );
  await userEvent.click(await screen.findByRole('button', { name: 'Usage Agent' }));
  await userEvent.click(screen.getByRole('menuitemradio', { name: 'Research Agent (agt-1)' }));
  expect(screen.getByRole('button', { name: 'Agent: Research Agent' })).toBeVisible();
  expect(new URLSearchParams(window.location.search).get('agent')).toBeNull();
  let finish!: (value: ReturnType<typeof reportFixture>) => void;
  let wanted!: UsageMetricQuery;
  client.usageMetrics.create.mockImplementationOnce((query: UsageMetricQuery) => {
    wanted = query;
    return new Promise((resolve) => {
      finish = resolve;
    });
  });
  await userEvent.click(screen.getByRole('button', { name: 'Apply usage filters' }));
  await waitFor(() => expect(finish).toBeDefined());
  expect(screen.queryByRole('button', { name: 'Agent: Research Agent' })).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Sandbox: Research Sandbox' })).toBeVisible();
  expect(wanted.filters).toMatchObject({ sandbox_id: 'sbx-1', agent_id: 'agt-1' });
  await act(async () => finish(snapshot(wanted, 'agent-selected')));
  await screen.findByRole('button', { name: 'Agent: Research Agent' });
  rerender(<UsageDialog {...props} open={false} />);
  rerender(<UsageDialog {...props} open />);
  expect(screen.getByRole('button', { name: 'Agent: Research Agent' })).toBeVisible();
  expect(screen.getByRole('button', { name: 'Sandbox: Research Sandbox' })).toBeVisible();
  expect(client.agents.read).not.toHaveBeenCalled();
  expect(client.usageMetrics.sandbox).not.toHaveBeenCalled();
  expect(client.usageMetrics.options).not.toHaveBeenCalled();
  expect(window.location.search).toBe('?sandbox=sbx-1&agent=agt-1');
  await userEvent.click(screen.getByRole('button', { name: 'Clear Sandbox' }));
  expect(screen.queryByRole('button', { name: /^Agent:/ })).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: 'Apply usage filters' }));
  expect(window.location.search).toBe('');
});

it('recovers URL names only after the matching owner report commits and aborts when closed', async () => {
  window.history.replaceState(null, '', '/usage?sandbox=sbx-1&agent=agt-1');
  const client = clientFixture();
  let finish!: (value: ReturnType<typeof reportFixture>) => void;
  let query!: UsageMetricQuery;
  client.usageMetrics.create.mockImplementationOnce((wanted: UsageMetricQuery) => {
    query = wanted;
    return new Promise((resolve) => {
      finish = resolve;
    });
  });
  let resolveAgent!: (value: { id: string; sandboxId: string; name: string }) => void;
  client.agents.read.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        resolveAgent = resolve;
      }),
  );
  const props = { client, principal, onClose: vi.fn() };
  const { rerender } = render(<UsageDialog {...props} open />);
  await waitFor(() => expect(finish).toBeDefined());
  expect(client.usageMetrics.sandbox).not.toHaveBeenCalled();
  expect(client.agents.read).not.toHaveBeenCalled();
  expect(client.agents.list).not.toHaveBeenCalled();
  await act(async () => finish(snapshot(query, 'authorized-report')));
  await screen.findByRole('button', { name: 'Sandbox: Research Sandbox' });
  await waitFor(() => expect(client.agents.read).toHaveBeenCalledOnce());
  expect(screen.getByRole('button', { name: 'Agent: Loading Agent…' })).toBeVisible();
  const signal = client.agents.read.mock.calls[0]![1].signal as AbortSignal;
  rerender(<UsageDialog {...props} open={false} />);
  expect(signal.aborted).toBe(true);
  await act(async () =>
    resolveAgent({ id: 'agt-1', sandboxId: 'sbx-1', name: 'Discarded late name' }),
  );
  rerender(<UsageDialog {...props} open />);
  await screen.findByRole('button', { name: 'Agent: Research Agent' });
  expect(screen.queryByText('Discarded late name')).not.toBeInTheDocument();
  expect(client.usageMetrics.sandbox).toHaveBeenCalledOnce();
  expect(client.agents.read).toHaveBeenCalledTimes(2);
  expect(client.agents.list).not.toHaveBeenCalled();
});

it.each(['owner', 'capability epoch'])(
  'drops cached names immediately on %s changes and rejects a late old response',
  async (change) => {
    window.history.replaceState(null, '', '/usage?sandbox=sbx-1&agent=agt-1');
    const client = clientFixture();
    let finish!: (value: { id: string; sandboxId: string; name: string }) => void;
    client.agents.read.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
    const props = { client, open: true, onClose: vi.fn() };
    const { rerender } = render(<UsageDialog {...props} principal={principal} />);
    await screen.findByRole('button', { name: 'Sandbox: Research Sandbox' });
    await waitFor(() => expect(finish).toBeDefined());
    const signal = client.agents.read.mock.calls[0]![1].signal as AbortSignal;
    client.usageMetrics.create.mockRejectedValue(new ApiError(403, 'No access'));
    const next = {
      principal: change === 'owner' ? 'owner-2' : principal.principal,
      cacheKey: 'next-epoch',
    };
    rerender(<UsageDialog {...props} principal={next} />);
    expect(
      screen.queryByRole('button', { name: 'Sandbox: Research Sandbox' }),
    ).not.toBeInTheDocument();
    expect(signal.aborted).toBe(true);
    await act(async () => finish({ id: 'agt-1', sandboxId: 'sbx-1', name: 'Old private Agent' }));
    await screen.findByRole('button', { name: 'Sandbox: Selected Sandbox unavailable' });
    expect(screen.queryByText('Old private Agent')).not.toBeInTheDocument();
    expect(client.agents.read).toHaveBeenCalledOnce();
    expect(client.agents.list).not.toHaveBeenCalled();
  },
);

it.each(['refused', 'scope mismatch', 'wrong Sandbox', 'empty report'])(
  'does not resolve names or enumerate Agents under a %s report',
  async (reason) => {
    window.history.replaceState(null, '', '/usage?sandbox=sbx-1&agent=agt-1');
    const client = clientFixture();
    client.usageMetrics.create.mockImplementation(async (query: UsageMetricQuery) => {
      if (reason === 'refused') throw new ApiError(404, 'Not owned');
      const report = snapshot(query, 'report', reason === 'scope mismatch' ? 'owner-2' : 'owner-1');
      if (reason === 'wrong Sandbox') report.query.filters.sandbox_id = 'different';
      return reason === 'empty report'
        ? { ...report, summary: { ...report.summary!, sandbox_count: 0 } }
        : report;
    });
    render(<UsageDialog client={client} open onClose={vi.fn()} principal={principal} />);
    await waitFor(() => expect(client.usageMetrics.create).toHaveBeenCalledOnce());
    await act(async () => {});
    expect(client.agents.read).not.toHaveBeenCalled();
    expect(client.agents.list).not.toHaveBeenCalled();
    expect(client.usageMetrics.sandbox).not.toHaveBeenCalled();
    expect(screen.queryByRole('button', { name: /^Agent:/ })).not.toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: 'Sandbox: Selected Sandbox unavailable' }),
    ).toBeInTheDocument();
    expect(window.location.search).toBe('?sandbox=sbx-1&agent=agt-1');
  },
);
it('never discovers Agents when the owner Sandbox report is refused', async () => {
  window.history.replaceState(null, '', '/usage?sandbox=shared-sandbox&owner=someone-else');
  const api = metricsClient(false);
  api.create.mockRejectedValue(new ApiError(404, 'Not owned'));
  const agents = { list: vi.fn(), read: vi.fn() };
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
