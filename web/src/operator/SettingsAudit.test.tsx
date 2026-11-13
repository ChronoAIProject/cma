import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { ApiError } from '@chronoai/cma-sdk';
import { SettingsAudit } from './SettingsAudit';
import { operatorClient, now } from './testFixtures';

const record = {
  id: 'activity-fixture',
  at: now,
  actor: 'fixture-operator',
  actor_name: 'Test administrator',
  action: 'configure' as const,
  target: { kind: 'resource' as const, id: 'platform_settings' },
  outcome: { status: 'succeeded' as const },
  detail: { revision: '4', private: 'not shown' },
};
describe('settings revision audit', () => {
  it('shows the current revision author/time from a bounded lookup', async () => {
    const client = operatorClient();
    client.admin.listActivity.mockResolvedValue({ items: [record], next: 'older' });
    const { container } = render(<SettingsAudit client={client} revision={4} allowed />);
    await screen.findByText(/Last platform settings change: Test administrator/);
    expect(container.querySelector('time')).toHaveAttribute('datetime', now);
    expect(client.admin.listActivity).toHaveBeenCalledWith({
      action: 'configure',
      limit: 50,
      signal: expect.any(AbortSignal),
    });
    expect(client.admin.listActivity).toHaveBeenCalledTimes(1);
    expect(screen.queryByText('not shown')).not.toBeInTheDocument();
  });
  it('does not request activity without its separate permission', () => {
    const client = operatorClient();
    render(<SettingsAudit client={client} revision={4} allowed={false} />);
    expect(screen.getByText(/require activity-log permission/)).toBeInTheDocument();
    expect(client.admin.listActivity).not.toHaveBeenCalled();
  });
  it('does not attribute stale, failed or unrelated operations to the current revision', async () => {
    const client = operatorClient();
    client.admin.listActivity.mockResolvedValue({
      items: [
        { ...record, detail: { revision: '3' } },
        { ...record, outcome: { status: 'refused', code: 'forbidden' } },
        { ...record, target: { kind: 'resource', id: 'unrelated' } },
      ],
      next: 'older',
    });
    render(<SettingsAudit client={client} revision={4} allowed />);
    await screen.findByText(/not in the recent activity page/);
    expect(screen.queryByText(/Test administrator/)).not.toBeInTheDocument();
    expect(client.admin.listActivity).toHaveBeenCalledTimes(1);
  });
  it.each([403, 503])('shows activity lookup failure %s explicitly', async (status) => {
    const client = operatorClient();
    client.admin.listActivity.mockRejectedValue(new ApiError(status, 'Activity unavailable'));
    render(<SettingsAudit client={client} revision={4} allowed />);
    await screen.findByText(/Last change details could not be read/);
  });
  it('does not substitute a raw account identifier for a missing name', async () => {
    const client = operatorClient();
    client.admin.listActivity.mockResolvedValue({
      items: [{ ...record, actor_name: null }],
      next: null,
    });
    render(<SettingsAudit client={client} revision={4} allowed />);
    await screen.findByText(/Name unavailable/);
    expect(screen.queryByText(/fixture-operator/)).not.toBeInTheDocument();
  });
});
