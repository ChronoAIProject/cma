import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { BridgeCredential } from './BridgeCredential';
import { bridgeDetail, bridgeTransport } from './fixtures.test-support';

it('requires deliberate background authority consent and names its exact owner and lifetime', async () => {
  const bodies: unknown[] = [];
  const test = bridgeTransport((url, init) => {
    if (!url.pathname.endsWith('/credential')) return undefined;
    bodies.push(JSON.parse(String(init.body)));
    return Response.json({ revision: 1, present: true, revoked: false });
  });
  const detail = {
    ...bridgeDetail,
    bridge: {
      ...bridgeDetail.bridge,
      owner: { kind: 'organization' as const, subject: 'org-research' },
    },
  };
  render(<BridgeCredential client={test.client} detail={detail} allowed onChanged={vi.fn()} />);
  const consent = screen.getByRole('checkbox', { name: /use my NyxID connection for 30 days/ });
  expect(consent).not.toBeChecked();
  expect(screen.getByText(/organization org-research/)).toHaveTextContent(
    'continues after browser logout',
  );
  const user = userEvent.setup();
  for (const explicit of [false, true]) {
    if (explicit) await user.click(consent);
    await user.type(screen.getByLabelText('Import selected Agent key'), 'fixture-selected-secret');
    await user.click(screen.getByRole('button', { name: 'Import key' }));
    await waitFor(() => expect(bodies).toHaveLength(explicit ? 2 : 1));
    expect(bodies.at(-1)).toMatchObject({ retain_control_plane_authority: explicit });
    expect(screen.getByLabelText('Import selected Agent key')).toHaveValue('');
  }
  expect(localStorage.length).toBe(0);
  expect(sessionStorage.length).toBe(0);
});

it('clears secret and consent when the actor client, bridge revision or management access changes', async () => {
  const first = bridgeTransport();
  const second = bridgeTransport();
  const view = render(
    <BridgeCredential client={first.client} detail={bridgeDetail} allowed onChanged={vi.fn()} />,
  );
  const user = userEvent.setup();
  const input = () => screen.getByLabelText('Import selected Agent key');
  const consent = () =>
    screen.getByRole('checkbox', { name: /use my NyxID connection for 30 days/ });
  await user.type(input(), 'fixture-secret');
  await user.click(consent());
  view.rerender(
    <BridgeCredential client={second.client} detail={bridgeDetail} allowed onChanged={vi.fn()} />,
  );
  expect(input()).toHaveValue('');
  expect(consent()).not.toBeChecked();
  await user.type(input(), 'fixture-secret');
  await user.click(consent());
  view.rerender(
    <BridgeCredential
      client={second.client}
      detail={bridgeDetail}
      allowed={false}
      onChanged={vi.fn()}
    />,
  );
  expect(input()).toHaveValue('');
  expect(consent()).not.toBeChecked();
  expect(consent()).toBeDisabled();
});
