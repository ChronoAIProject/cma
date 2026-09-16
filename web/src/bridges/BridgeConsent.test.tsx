import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import type { BridgeInvokeGrant } from '@chronoai/cma-sdk';
import { BridgeManager } from './BridgeManager';
import { bridgeDetail, bridgeTransport } from './fixtures.test-support';
import { NOW } from '../test/rest';

it('requires a new context review when periodic refresh replaces the saved configuration', async () => {
  const intervals: (() => void)[] = [];
  const realInterval = globalThis.setInterval;
  const timer = vi.spyOn(globalThis, 'setInterval').mockImplementation(((
    callback: TimerHandler,
    delay?: number,
    ...args: unknown[]
  ) => {
    if (delay === 30_000 && typeof callback === 'function') intervals.push(callback as () => void);
    return realInterval(callback, delay, ...args);
  }) as typeof setInterval);
  let revision = 1;
  const test = bridgeTransport((url) => {
    if (url.pathname.endsWith('/configuration'))
      return Response.json({
        ...bridgeDetail,
        bridge: { ...bridgeDetail.bridge, revision, config_revision: revision },
      });
    if (url.pathname.endsWith('/invoke-readiness'))
      return Response.json({
        bridge_id: 'brg_one',
        revision,
        config_revision: revision,
        status: 'consent_required',
        candidate: null,
      });
    return undefined;
  });
  try {
    render(<BridgeManager client={test.client} id="brg_one" allowed onChanged={vi.fn()} />);
    const checkbox = await screen.findByRole('checkbox', { name: /I authorize this bridge/ });
    await waitFor(() => expect(checkbox).toBeEnabled());
    fireEvent.click(checkbox);
    expect(checkbox).toBeChecked();
    revision = 2;
    await act(async () => {
      intervals.forEach((refresh) => refresh());
    });
    await waitFor(() =>
      expect(screen.getByRole('checkbox', { name: /I authorize this bridge/ })).not.toBeChecked(),
    );
    expect(screen.getByRole('button', { name: 'Grant Agent permission' })).toBeDisabled();
  } finally {
    timer.mockRestore();
  }
});

it('requires explicit retained-context permission after connection validation and reflects terminal revocation', async () => {
  const target = bridgeDetail.config.placement;
  if (target.kind !== 'existing_agent') throw new Error('Expected retained fixture');
  let grant: BridgeInvokeGrant | undefined;
  const writes: { path: string; body: unknown }[] = [];
  const actor = { kind: 'human' as const, subject: 'owner' };
  const test = bridgeTransport((url, init) => {
    if (init.method === 'POST')
      writes.push({ path: url.pathname, body: JSON.parse(String(init.body)) as unknown });
    if (url.pathname.endsWith('/validate'))
      return Response.json({ bridge_id: 'brg_one', revision: 1, config_revision: 1, valid: true });
    if (url.pathname.endsWith('/invoke-readiness'))
      return Response.json({
        bridge_id: 'brg_one',
        revision: 1,
        config_revision: 1,
        status: grant?.state.kind === 'active' ? 'consent_present' : 'consent_required',
        candidate: grant?.state.kind === 'active' ? grant : null,
      });
    if (url.pathname.endsWith('/invoke-grants')) {
      if (init.method !== 'POST')
        return Response.json({ items: grant === undefined ? [] : [grant], next_cursor: null });
      grant = {
        id: `brinv_${'a'.repeat(64)}`,
        revision: 1,
        owner: bridgeDetail.bridge.owner,
        pin: { bridge_id: 'brg_one', config_revision: 1 },
        target: {
          ids: target.target,
          sandbox_created_at: NOW,
          workspace_created_at: NOW,
          agent_created_at: NOW,
          agent_creation_fingerprint: 'b'.repeat(64),
          worktree_identity_digest: 'c'.repeat(64),
        },
        consent: 'use_retained_agent_context',
        issued_at: NOW,
        issued_by: actor,
        state: { kind: 'active' },
      };
      return Response.json(
        {
          id: 'd'.repeat(64),
          request_digest: 'e'.repeat(64),
          bridge_id: 'brg_one',
          actor,
          result: grant,
        },
        { status: 201 },
      );
    }
    if (url.pathname.endsWith('/revoke') && grant !== undefined) {
      grant = { ...grant, revision: 2, state: { kind: 'revoked', at: NOW, actor } };
      return Response.json({
        id: 'f'.repeat(64),
        request_digest: 'e'.repeat(64),
        bridge_id: 'brg_one',
        actor,
        result: grant,
      });
    }
    return undefined;
  });
  render(<BridgeManager client={test.client} id="brg_one" allowed onChanged={vi.fn()} />);
  const user = userEvent.setup();
  await user.click(await screen.findByRole('button', { name: 'Validate connection' }));
  await screen.findByText(/selected connection is verified/);
  expect(screen.getByRole('button', { name: 'Enable bridge' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Grant Agent permission' })).toBeDisabled();
  await user.click(screen.getByRole('checkbox', { name: /I authorize this bridge/ }));
  await user.click(screen.getByRole('button', { name: 'Grant Agent permission' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Enable bridge' })).toBeEnabled());
  expect(writes.at(-1)?.body).toEqual({
    expected_bridge_revision: 1,
    target: target.target,
    shared_context: 'use_retained_agent_context',
  });
  await user.click(await screen.findByRole('button', { name: 'Revoke permission' }));
  await user.click(screen.getByRole('button', { name: 'Revoke permission' }));
  await screen.findByText('Revoked permission');
  expect(screen.getByRole('button', { name: 'Enable bridge' })).toBeDisabled();
  expect(writes.at(-1)?.body).toEqual({ expected_bridge_revision: 1, expected_revision: 1 });
  expect(writes.every(({ path }) => !path.endsWith('/commands'))).toBe(true);
});
