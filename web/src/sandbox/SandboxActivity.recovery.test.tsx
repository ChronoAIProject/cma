import { render, screen, waitFor } from '@testing-library/react';
import { expect, it } from 'vitest';
import { createRestWebClient } from '../api/rest';
import { NOW, restTransport } from '../test/rest';
import { SandboxActivity } from './SandboxActivity';

const event = (seq: number, kind: Record<string, unknown>) => ({
  sandbox_id: 'sandbox-1',
  seq,
  appended_at: NOW,
  kind,
});

it('reads recovered runtime history through the public client after remounting', async () => {
  const transport = restTransport();
  const client = createRestWebClient(transport.sdk);
  const history = [
    event(1, { type: 'status_changed', from: 'active', to: 'rotating' }),
    event(2, {
      type: 'runtime_recovery_admitted',
      previous_status: 'rotating',
      target_runtime_version: 'runtime-v2',
      proof: { provider_plan: { private_canary: 'synthetic-private-recovery' } },
    }),
    event(3, {
      type: 'runtime_recovery_quiesced',
      receipt: { provider_receipt: { private_canary: 'synthetic-private-recovery' } },
    }),
    event(4, { type: 'rotated', runtime_version: 'runtime-v2' }),
    event(5, { type: 'status_changed', from: 'rotating', to: 'active' }),
  ];
  for (let mount = 0; mount < 2; mount += 1) {
    transport.reply(history);
    transport.reply([]);
    const view = render(<SandboxActivity client={client} sandboxId="sandbox-1" settled />);
    await screen.findByText('Sandbox updated');
    expect(screen.getByText('Sandbox runtime recovery requested')).toBeInTheDocument();
    expect(screen.getByText('Sandbox runtime recovery ready')).toBeInTheDocument();
    expect(screen.getByText('Sandbox: Live')).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByText('Syncing recorded activity.')).toBeNull());
    expect(screen.queryByText('No activity recorded yet.')).toBeNull();
    expect(view.container.textContent).not.toContain('synthetic-private-recovery');
    expect(transport.fetch.mock.calls.at(-1)?.[0]).toContain('after=5');
    view.unmount();
  }
  expect(transport.fetch).toHaveBeenCalledTimes(4);
});

it('keeps a malformed recovery page unconfirmed without advancing its cursor', async () => {
  const transport = restTransport();
  transport.reply([
    event(1, {
      type: 'runtime_recovery_admitted',
      previous_status: 'rotating',
      target_runtime_version: 42,
    }),
  ]);
  render(
    <SandboxActivity client={createRestWebClient(transport.sdk)} sandboxId="sandbox-1" settled />,
  );
  await screen.findByText('Activity updates interrupted. Reconnecting.');
  expect(screen.queryByText('Sandbox runtime recovery requested')).toBeNull();
  expect(transport.fetch).toHaveBeenCalledTimes(1);
});
