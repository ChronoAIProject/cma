import { act, render, renderHook, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { BridgePage } from './BridgePage';
import { useBridgeCreation } from './useBridgeCreation';
import {
  bridgeChoices,
  bridgeConfig,
  bridgeCreationTarget,
  bridgeTransport,
} from './fixtures.test-support';
import { chooseOption } from '../test/select';
import { NOW } from '../test/rest';

const request = { owner: bridgeCreationTarget.owner, config: bridgeConfig };
const receipt = {
  bridge_id: bridgeCreationTarget.bridge_id,
  owner: bridgeCreationTarget.owner,
  actor: { kind: 'human', subject: 'owner' },
  revision: 1,
  config_revision: 1,
  command: 'create',
  applied_at: NOW,
};
const key = (init: RequestInit) => new Headers(init.headers).get('Idempotency-Key');

it('shows the callback before any connection exists and refreshes choices without replacing its key', async () => {
  let connected = false;
  const writes: RequestInit[] = [];
  const previews: RequestInit[] = [];
  const test = bridgeTransport((url, init) => {
    if (url.pathname.endsWith('/bridge-creation-target')) previews.push(init);
    if (url.pathname.endsWith('/bridge-connectors'))
      return Response.json(connected ? bridgeChoices : { bots: [], keys: [], routes: [] });
    if (url.pathname.endsWith('/bridges') && init.method === 'POST') {
      writes.push(init);
      return Response.json(receipt, { status: 201 });
    }
    return undefined;
  });
  const navigate = vi.fn();
  render(
    <BridgePage
      client={test.client}
      viewerId="owner"
      route={{ kind: 'bridge_new' }}
      onNavigate={navigate}
    />,
  );
  await screen.findByText(bridgeCreationTarget.callback_url);
  expect(writes).toHaveLength(0);
  expect(screen.getByRole('button', { name: 'Save disabled bridge' })).toBeDisabled();
  const user = userEvent.setup();
  connected = true;
  await user.click(screen.getByRole('button', { name: 'Refresh connection choices' }));
  await waitFor(() => expect(screen.getByLabelText('Bridge name')).toBeEnabled());
  await user.type(screen.getByLabelText('Bridge name'), 'New personal bridge');
  await chooseOption(user, 'Existing channel bot', 'telegram · bot-one');
  await chooseOption(user, 'Existing channel key', 'key-one');
  await user.click(screen.getByRole('checkbox', { name: 'route-one' }));
  await chooseOption(user, 'Resource placement', 'One retained Agent');
  await waitFor(() => expect(screen.getByRole('button', { name: 'Retained Agent' })).toBeEnabled());
  await chooseOption(user, 'Retained Agent', 'Agent one');
  await user.click(screen.getByRole('checkbox', { name: /I reviewed who owns/ }));
  await user.click(screen.getByRole('button', { name: 'Save disabled bridge' }));
  await waitFor(() =>
    expect(navigate).toHaveBeenCalledWith({ kind: 'bridge', bridgeId: receipt.bridge_id }),
  );
  expect(key(writes[0]!)).toBe(key(previews[0]!));
  expect(sessionStorage.length).toBe(0);
});

it('retains the setup callback and creation key when returning or reloading before save', async () => {
  const test = bridgeTransport();
  const props = {
    client: test.client,
    viewerId: 'owner',
    route: { kind: 'bridge_new' as const },
    onNavigate: vi.fn(),
  };
  const first = render(<BridgePage {...props} />);
  await screen.findByText(bridgeCreationTarget.callback_url);
  first.unmount();
  render(<BridgePage {...props} />);
  await screen.findByText(bridgeCreationTarget.callback_url);
  const previews = test.fetch.mock.calls.filter(([url]) =>
    String(url).includes('/bridge-creation-target'),
  );
  expect(previews).toHaveLength(2);
  expect(key(previews[0]![1]!)).toBe(key(previews[1]![1]!));
  expect(test.fetch.mock.calls.every(([, init]) => init?.method === 'GET')).toBe(true);
});

it('replays an uncertain create after reload with its exact body and key, only on explicit recovery', async () => {
  const writes: RequestInit[] = [];
  const test = bridgeTransport((url, init) => {
    if (url.pathname.endsWith('/bridges') && init.method === 'POST') {
      writes.push(init);
      return writes.length === 1
        ? Promise.reject(new TypeError('connection lost'))
        : Response.json(receipt, { status: 201 });
    }
    return undefined;
  });
  const first = renderHook(() => useBridgeCreation(test.client, 'owner'));
  await waitFor(() => expect(first.result.current.ready).toBe(true));
  await act(async () => {
    await first.result.current.submit(request);
  });
  expect(first.result.current.pending).toBe(true);
  first.unmount();
  const navigate = vi.fn();
  render(
    <BridgePage
      client={test.client}
      viewerId="owner"
      route={{ kind: 'bridge_new' }}
      onNavigate={navigate}
    />,
  );
  await screen.findByText(bridgeCreationTarget.callback_url);
  expect(writes).toHaveLength(1);
  expect(screen.getByLabelText('Bridge name')).toBeDisabled();
  await userEvent.setup().click(screen.getByRole('button', { name: 'Recover saved bridge' }));
  await waitFor(() =>
    expect(navigate).toHaveBeenCalledWith({ kind: 'bridge', bridgeId: receipt.bridge_id }),
  );
  expect(writes[1]?.body).toBe(writes[0]?.body);
  expect(key(writes[1]!)).toBe(key(writes[0]!));
  expect(sessionStorage.length).toBe(0);
});

it('keeps recovery locked after a mismatched receipt or later refusal', async () => {
  let attempts = 0;
  const test = bridgeTransport((url, init) => {
    if (url.pathname.endsWith('/bridges') && init.method === 'POST') {
      attempts++;
      return attempts === 1
        ? Response.json({ ...receipt, bridge_id: `brg_${'b'.repeat(64)}` }, { status: 201 })
        : Response.json({ code: 'bridge_management_required' }, { status: 403 });
    }
    return undefined;
  });
  const view = renderHook(() => useBridgeCreation(test.client, 'owner'));
  await waitFor(() => expect(view.result.current.ready).toBe(true));
  let result: unknown;
  await act(async () => {
    result = await view.result.current.submit(request);
  });
  expect(result).toBeUndefined();
  expect(view.result.current.pending).toBe(true);
  await act(async () => {
    result = await view.result.current.submit();
  });
  expect(result).toBeUndefined();
  expect(view.result.current.pending).toBe(true);
});

it('hides the old callback when viewer or organization changes and uses separate creation keys', async () => {
  const callbacks = [
    bridgeCreationTarget,
    {
      ...bridgeCreationTarget,
      bridge_id: `brg_${'b'.repeat(64)}`,
      callback_url: `https://cma.example/brg_${'b'.repeat(64)}`,
      owner: { kind: 'person', subject: 'other' },
    },
  ];
  let viewer = 0;
  const test = bridgeTransport((url) =>
    url.pathname.endsWith('/bridge-creation-target')
      ? Response.json(
          url.searchParams.has('organization')
            ? { ...callbacks[1], owner: { kind: 'organization', subject: 'org-one' } }
            : callbacks[viewer],
        )
      : undefined,
  );
  const props = { client: test.client, onNavigate: vi.fn() };
  const view = render(<BridgePage {...props} viewerId="owner" route={{ kind: 'bridge_new' }} />);
  await screen.findByText(callbacks[0]!.callback_url);
  viewer = 1;
  view.rerender(<BridgePage {...props} viewerId="other" route={{ kind: 'bridge_new' }} />);
  expect(screen.queryByText(callbacks[0]!.callback_url)).not.toBeInTheDocument();
  await screen.findByText(callbacks[1]!.callback_url);
  view.rerender(
    <BridgePage
      {...props}
      viewerId="other"
      route={{ kind: 'bridge_new', organization: 'org-one' }}
    />,
  );
  expect(screen.queryByText(callbacks[1]!.callback_url)).not.toBeInTheDocument();
  await screen.findByText(callbacks[1]!.callback_url);
  const keys = test.fetch.mock.calls
    .filter(([url]) => String(url).includes('/bridge-creation-target'))
    .map(([, init]) => key(init!));
  expect(new Set(keys).size).toBe(3);
});

it.each(['malformed', 'denied'])('blocks creation when the preview is %s', async (kind) => {
  const test = bridgeTransport((url) =>
    url.pathname.endsWith('/bridge-creation-target')
      ? kind === 'malformed'
        ? Response.json({ ...bridgeCreationTarget, callback_url: 'http://unsafe.example/path' })
        : Response.json({ code: 'bridge_management_required' }, { status: 403 })
      : undefined,
  );
  render(
    <BridgePage
      client={test.client}
      viewerId="owner"
      route={{ kind: 'bridge_new' }}
      onNavigate={vi.fn()}
    />,
  );
  await screen.findByRole('button', { name: 'Check callback again' });
  expect(screen.getByRole('button', { name: 'Save disabled bridge' })).toBeDisabled();
  expect(screen.queryByText(bridgeCreationTarget.callback_url)).not.toBeInTheDocument();
  expect(test.fetch.mock.calls.every(([, init]) => init?.method === 'GET')).toBe(true);
});
