import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { operatorClient } from '../operator/testFixtures';
import { SandboxUsagePanel } from './SandboxUsagePanel';
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
