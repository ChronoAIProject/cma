import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { operatorClient } from '../operator/testFixtures';
import { SandboxUsagePanel } from './SandboxUsagePanel';
import { AdminSandboxUsage } from './AdminSandboxUsage';
import { chooseOptionValue } from '../test/select';
import { Settings } from './Settings';
import { parseRoute, routePath } from '../routing/routes';

const totals = {
  uptime_seconds: 3661,
  cpu_core_hours: 1.017,
  memory_gib_hours: 2.034,
  disk_gib_hours: 20.34,
  cpu_millis_seconds: 3661000,
  memory_mib_seconds: 7497728,
  disk_gib_seconds: 73224,
};
const summary = {
  as_of: 1793577600,
  measurement: 'allocated_resource_time' as const,
  sandbox_count: 1,
  running_count: 1,
  incomplete_lifecycle_count: 0,
  tracking_started_at: 1793573939,
  totals,
};
const row = {
  sandbox_id: 'sbx-research',
  owner: 'owner-1',
  name: 'Research',
  status: 'active',
  allocation: { running: true, cpu_millis: 1000, memory_mib: 2048, disk_gib: 20 },
  created_at: 1793573939,
  tracking_started_at: 1793573939,
  as_of: 1793577600,
  complete_lifecycle: true,
  totals,
};
function client() {
  const value = operatorClient().usage;
  value.summary.mockResolvedValue(summary);
  value.sandboxes.mockResolvedValue({ items: [row], next: null, as_of: summary.as_of });
  value.adminSummary.mockResolvedValue(summary);
  value.adminSandboxes.mockResolvedValue({ items: [row], next: null, as_of: summary.as_of });
  value.events.mockResolvedValue({ items: [], next: null });
  return value;
}

it('shows owner usage from the public owner endpoints and opens per-sandbox history', async () => {
  const api = client();
  const user = userEvent.setup();
  render(<SandboxUsagePanel client={api} />);
  await screen.findByRole('button', { name: 'Research' });
  expect(screen.getAllByText('1h 1m 1s')).toHaveLength(2);
  expect(screen.getByText('1.017 core-hours')).toBeInTheDocument();
  expect(api.adminSummary).not.toHaveBeenCalled();
  expect(api.adminSandboxes).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: 'Research' }));
  await waitFor(() =>
    expect(api.events).toHaveBeenCalledWith('sbx-research', expect.objectContaining({ limit: 50 })),
  );
});

it('combines user, status and Sandbox ID filters across pages and preserves owner choices', async () => {
  const api = operatorClient();
  api.usage = client();
  api.usage.adminSandboxes.mockResolvedValueOnce({
    items: [row],
    next: 'next-usage',
    as_of: summary.as_of,
  });
  api.usage.adminSandboxes.mockResolvedValueOnce({
    items: [
      {
        ...row,
        sandbox_id: 'sbx-archived',
        name: 'Archived',
        owner: 'former-owner',
        status: 'retired',
        allocation: { ...row.allocation, running: false },
      },
    ],
    next: null,
    as_of: summary.as_of,
  });
  const user = userEvent.setup();
  render(<AdminSandboxUsage client={api} />);
  await screen.findByRole('button', { name: 'Archived' });
  expect(api.usage.adminSandboxes).toHaveBeenLastCalledWith(
    expect.objectContaining({ cursor: 'next-usage', limit: 200 }),
  );
  await chooseOptionValue(user, screen.getByRole('button', { name: 'Usage user' }), 'former-owner');
  await chooseOptionValue(
    user,
    screen.getByRole('button', { name: 'Usage sandbox status' }),
    'retired',
  );
  await user.type(screen.getByRole('searchbox', { name: 'Sandbox ID' }), 'ARCH');
  await user.click(screen.getByRole('button', { name: 'Apply filters' }));
  expect(screen.queryByRole('button', { name: 'Research' })).not.toBeInTheDocument();
  expect(screen.getByText('1 (0 running)')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Archived' })).toBeInTheDocument();
  await user.type(screen.getByRole('searchbox', { name: 'Sandbox ID' }), '-missing');
  await user.click(screen.getByRole('button', { name: 'Apply filters' }));
  expect(screen.getByText('No sandbox usage matches these filters.')).toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Usage user' }));
  expect(screen.getByRole('menuitemradio', { name: 'Alex Chen' })).toBeInTheDocument();
  expect(
    screen.getByRole('menuitemradio', { name: 'Name unavailable (former-owner)' }),
  ).toBeInTheDocument();
  expect(api.usage.adminSandboxes).toHaveBeenCalledTimes(2);
  expect(api.usage.summary).not.toHaveBeenCalled();
});

it('sorts numeric columns across the entire inventory in both directions and resets pagination', async () => {
  const api = operatorClient();
  api.usage = client();
  const rows = Array.from({ length: 51 }, (_, index) => ({
    ...row,
    sandbox_id: `sbx-${String(index).padStart(3, '0')}`,
    name: `Sandbox ${index}`,
    totals: { ...totals, cpu_millis_seconds: (index + 1) * 3_600_000, cpu_core_hours: index + 1 },
  }));
  api.usage.adminSandboxes.mockResolvedValueOnce({
    items: rows.slice(0, 50),
    next: 'next-usage',
    as_of: summary.as_of,
  });
  api.usage.adminSandboxes.mockResolvedValueOnce({
    items: rows.slice(50),
    next: null,
    as_of: summary.as_of,
  });
  const user = userEvent.setup();
  render(<AdminSandboxUsage client={api} />);
  await screen.findByText('51 matching Sandboxes · Page 1 of 2');
  await user.click(screen.getByRole('button', { name: 'Next page' }));
  expect(screen.getByRole('button', { name: 'Sandbox 50' })).toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Sort by CPU (core-hours) ascending' }));
  const table = screen.getByRole('table', { name: 'Usage by sandbox' });
  expect(screen.getByText('51 matching Sandboxes · Page 1 of 2')).toBeInTheDocument();
  expect(within(table).getAllByRole('row')[1]).toHaveTextContent('Sandbox 0');
  expect(screen.getByRole('columnheader', { name: /CPU \(core-hours\)/ })).toHaveAttribute(
    'aria-sort',
    'ascending',
  );
  await user.click(screen.getByRole('button', { name: 'Sort by CPU (core-hours) descending' }));
  expect(within(table).getAllByRole('row')[1]).toHaveTextContent('Sandbox 50');
  expect(screen.getByRole('columnheader', { name: /CPU \(core-hours\)/ })).toHaveAttribute(
    'aria-sort',
    'descending',
  );
  expect(api.usage.adminSandboxes).toHaveBeenCalledTimes(2);
});

it('does not display partial inventories as complete and retains successful rows after a failed refresh', async () => {
  const api = operatorClient();
  api.usage = client();
  api.usage.adminSandboxes.mockResolvedValueOnce({
    items: [row],
    next: 'stuck',
    as_of: summary.as_of,
  });
  api.usage.adminSandboxes.mockRejectedValueOnce(new Error('second page failed'));
  const user = userEvent.setup();
  render(<AdminSandboxUsage client={api} />);
  await screen.findByText('Usage unavailable');
  expect(screen.queryByRole('button', { name: 'Research' })).not.toBeInTheDocument();
  expect(screen.queryByText('0 core-hours')).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Refresh usage' }));
  await screen.findByRole('button', { name: 'Research' });
  api.usage.adminSandboxes.mockRejectedValueOnce(new Error('refresh failed'));
  await user.click(screen.getByRole('button', { name: 'Refresh usage' }));
  await screen.findByText('Usage unavailable');
  expect(screen.getByRole('button', { name: 'Research' })).toBeInTheDocument();
});

it('reports missing history and retries failed usage reads without presenting a zero balance', async () => {
  const api = client();
  const user = userEvent.setup();
  api.summary
    .mockRejectedValueOnce(new Error('unavailable'))
    .mockResolvedValue({ ...summary, incomplete_lifecycle_count: 1 });
  render(<SandboxUsagePanel client={api} />);
  await screen.findByText('Usage unavailable');
  expect(screen.queryByText('0 core-hours')).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Refresh usage' }));
  await screen.findByText('Some usage history is unavailable');
});

it('restores the Usage settings tab from its URL and supports tab keyboard navigation', async () => {
  const api = client();
  const navigate = vi.fn();
  const user = userEvent.setup();
  expect(parseRoute('/settings/usage')).toEqual({ kind: 'settings', tab: 'usage' });
  expect(routePath({ kind: 'settings', tab: 'usage' })).toBe('/settings/usage');
  render(<Settings client={api} tab="usage" onNavigate={navigate} />);
  await screen.findByRole('button', { name: 'Research' });
  const tab = screen.getByRole('tab', { name: 'Usage' });
  expect(tab).toHaveAttribute('aria-selected', 'true');
  tab.focus();
  await user.keyboard('{ArrowLeft}');
  expect(navigate).toHaveBeenCalledWith({ kind: 'settings', tab: 'general' });
});
