import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ConnectionWizard } from './ConnectionWizard';
import {
  subscriptionService,
  type ConnectionClient,
  type ConnectionReport,
  type ConnectionTarget,
  type StartedConnection,
} from './client';

const target: ConnectionTarget = { type: 'agent', id: 'agent-one' };
const services = ['api-github', 'ornn-api', subscriptionService];
const names = ['GitHub', 'Ornn skills', 'My Codex subscription'];
const flush = () => act(async () => undefined);
const poll = () =>
  act(async () => {
    await vi.advanceTimersByTimeAsync(2000);
  });
let popup: {
  opener: unknown;
  closed: boolean;
  location: { href: string };
  focus: ReturnType<typeof vi.fn>;
  close: ReturnType<typeof vi.fn>;
};

beforeEach(() => {
  vi.useFakeTimers();
  sessionStorage.clear();
  popup = { opener: {}, closed: false, location: { href: '' }, focus: vi.fn(), close: vi.fn() };
  vi.spyOn(window, 'open').mockImplementation(() => popup as unknown as Window);
});
afterEach(() => {
  vi.unstubAllEnvs();
  vi.unstubAllGlobals();
  vi.useRealTimers();
  vi.restoreAllMocks();
  sessionStorage.clear();
});

function fixture() {
  const connected = new Set<string>();
  const report = (owner = 'owner'): ConnectionReport => ({
    owner,
    ready: connected.size === services.length,
    checked_at: new Date().toISOString(),
    services: services.map((service, index) => ({
      service,
      display_name: names[index] ?? service,
      kind: index === 2 ? 'codex' : 'nyxid',
      state: connected.has(service) ? 'satisfied' : 'missing',
      connection_mode: 'hosted',
    })),
  });
  const client: ConnectionClient = {
    check: vi.fn(async () => report()),
    start: vi.fn(async (_target, active) => ({
      ...active,
      owner: 'owner',
      hosted_url: `https://identity.example.test/${active.service_id}`,
      expires_at: new Date(Date.now() + 600000).toISOString(),
    })),
    cancel: vi.fn(async () => undefined),
  };
  return { client, connected, report };
}

it.each([false, true])(
  'cancels a hosted start acknowledged after manual GitHub repair (recovery=%s)',
  async (recovery) => {
    vi.stubEnv('VITE_CONNECTION_INTENT_RECOVERY_ENABLED', String(recovery));
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => Response.json({ owner: 'owner' })),
    );
    const { client, report } = fixture();
    let resolve!: (result: StartedConnection) => void;
    vi.mocked(client.start).mockImplementationOnce(
      () => new Promise((complete) => (resolve = complete)),
    );
    const ready = vi.fn();
    render(<ConnectionWizard target={target} client={client} repositoryRecovery onReady={ready} />);
    await flush();
    fireEvent.click(screen.getByRole('button', { name: 'Connect all' }));
    const active = vi.mocked(client.start).mock.calls[0]![1];
    vi.mocked(client.check).mockResolvedValue({
      ...report(),
      services: report().services.map((row) =>
        row.service === active.service_id ? { ...row, connection_mode: 'manual' } : row,
      ),
    });
    await poll();
    expect(screen.getByRole('button', { name: 'Verify again' })).toBeVisible();
    vi.mocked(client.cancel).mockClear();
    const hosted = {
      ...active,
      owner: 'owner',
      hosted_url: 'https://identity.example.test/github',
      expires_at: new Date(Date.now() + 60_000).toISOString(),
    };
    await act(async () => resolve(hosted));
    expect(client.cancel).toHaveBeenCalledExactlyOnceWith(target, hosted);
    const key = JSON.stringify(target);
    expect(sessionStorage.getItem(`cma.connection.${key}`)).toBeNull();
    const recoveryState = sessionStorage.getItem(`cma.connection-recovery.v1.${key}:false`);
    expect(JSON.parse(recoveryState ?? '{}')).not.toHaveProperty('pending');
    expect(popup.location.href).toBe('');
    expect(ready).not.toHaveBeenCalled();
    expect(screen.getByRole('button', { name: 'Verify again' })).toBeVisible();
  },
);

it('reuses the unacknowledged connection intent after a lost start response', async () => {
  vi.stubEnv('VITE_CONNECTION_INTENT_RECOVERY_ENABLED', 'true');
  const { client, connected } = fixture();
  services.slice(1).forEach((service) => connected.add(service));
  vi.mocked(client.start).mockRejectedValueOnce(new Error('Synthetic start response lost'));
  const ready = vi.fn();
  render(<ConnectionWizard target={target} client={client} onReady={ready} />);
  await flush();
  fireEvent.click(screen.getByRole('button', { name: 'Connect all' }));
  await flush();
  expect(screen.getByRole('alert')).toHaveTextContent('Synthetic start response lost');
  expect(client.start).toHaveBeenCalledOnce();
  expect(ready).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Connect all' }));
  await flush();
  const calls = vi.mocked(client.start).mock.calls;
  expect(calls).toHaveLength(2);
  expect(calls[1]?.[0]).toEqual(calls[0]?.[0]);
  expect(calls[1]?.[1].service_id).toBe(calls[0]?.[1].service_id);
  expect(calls[1]?.[1].intent_id).toBe(calls[0]?.[1].intent_id);
  expect(ready).not.toHaveBeenCalled();
});

it('verifies a connected session without opening a dialog or taking focus', async () => {
  const { client, connected } = fixture();
  services.forEach((service) => connected.add(service));
  const ready = vi.fn();
  render(
    <>
      <button autoFocus>Keep focus</button>
      <ConnectionWizard target={target} client={client} presentation="dialog" onReady={ready} />
    </>,
  );
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  await flush();
  expect(ready).toHaveBeenCalledOnce();
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Keep focus' })).toHaveFocus();
  expect(window.open).not.toHaveBeenCalled();
});

it('puts missing connections in a modal and returns focus after verification', async () => {
  const { client, connected } = fixture();
  const ready = vi.fn();
  render(
    <>
      <button autoFocus>Return here</button>
      <ConnectionWizard target={target} client={client} presentation="dialog" onReady={ready} />
    </>,
  );
  await flush();
  expect(screen.getByRole('dialog', { name: 'Connect required services' })).toHaveAttribute(
    'aria-modal',
    'true',
  );
  expect(screen.getByRole('button', { name: 'Close' })).toHaveFocus();
  services.forEach((service) => connected.add(service));
  await poll();
  expect(ready).toHaveBeenCalledOnce();
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Return here' })).toHaveFocus();
});

it('cancels the active server intent when the connection modal is dismissed', async () => {
  const { client } = fixture();
  const cancel = vi.fn();
  render(
    <ConnectionWizard
      target={target}
      client={client}
      presentation="dialog"
      onReady={vi.fn()}
      onCancel={cancel}
    />,
  );
  await flush();
  fireEvent.click(screen.getByRole('button', { name: 'Connect all' }));
  await flush();
  fireEvent.keyDown(document, { key: 'Escape' });
  await flush();
  expect(client.cancel).toHaveBeenCalledOnce();
  expect(popup.close).toHaveBeenCalledOnce();
  expect(cancel).toHaveBeenCalledOnce();
});

it('one Connect all action verifies every service in sequence before continuing once', async () => {
  const { client, connected } = fixture();
  const ready = vi.fn();
  render(<ConnectionWizard target={target} client={client} onReady={ready} />);
  await flush();
  fireEvent.click(screen.getByRole('button', { name: 'Connect all' }));
  await flush();
  expect(popup.location.href).toContain('api-github');
  expect(ready).not.toHaveBeenCalled();
  for (const service of services) {
    // Returning from one NyxID service must leave the same window reusable.
    popup.location.href = '/connection-return?status=completed';
    expect(popup.close).not.toHaveBeenCalled();
    connected.add(service);
    await poll();
  }
  expect(vi.mocked(client.start).mock.calls.map((call) => call[1].service_id)).toEqual(services);
  expect(ready).toHaveBeenCalledOnce();
  expect(window.open).toHaveBeenCalledOnce();
  expect(popup.close).toHaveBeenCalledOnce();
  await poll();
  expect(ready).toHaveBeenCalledOnce();
});

it('keeps checking after a popup is blocked and provides an explicit reopen action', async () => {
  vi.mocked(window.open).mockReturnValueOnce(null);
  const { client, connected } = fixture();
  const ready = vi.fn();
  render(<ConnectionWizard target={target} client={client} onReady={ready} />);
  await flush();
  fireEvent.click(screen.getByRole('button', { name: 'Connect all' }));
  await flush();
  expect(screen.getByText(/Open the sign-in window to continue/)).toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Open sign-in window' }));
  await flush();
  expect(client.start).toHaveBeenCalledOnce();
  expect(popup.location.href).toContain('api-github');
  services.forEach((service) => connected.add(service));
  await poll();
  expect(ready).toHaveBeenCalledOnce();
});

it('treats a completed callback as inert until the approved target is ready for the same owner', async () => {
  const { client, connected, report } = fixture();
  const accessTarget: ConnectionTarget = { type: 'agent_access_request', id: 'access-one' };
  const ready = vi.fn();
  render(
    <ConnectionWizard
      target={accessTarget}
      expectedOwner="owner"
      client={client}
      onReady={ready}
    />,
  );
  await flush();
  fireEvent.click(screen.getByRole('button', { name: 'Connect all' }));
  await flush();

  // Callback query parameters and closing the window cannot prove a connection.
  popup.location.href = '/connection-return?status=completed&connect_link_id=untrusted';
  popup.closed = true;
  await poll();
  expect(ready).not.toHaveBeenCalled();
  expect(client.check).toHaveBeenLastCalledWith(
    accessTarget,
    expect.objectContaining({ service_id: 'api-github', intent_id: expect.any(String) }),
    expect.any(AbortSignal),
  );

  services.forEach((service) => connected.add(service));
  vi.mocked(client.check).mockResolvedValue(report('different-owner'));
  await poll();
  expect(ready).not.toHaveBeenCalled();
  expect(screen.getByRole('alert')).toHaveTextContent('Your CMA account changed');

  vi.mocked(client.check).mockResolvedValue(report());
  await act(async () => vi.advanceTimersByTimeAsync(4000));
  expect(ready).toHaveBeenCalledOnce();
  expect(client.start).toHaveBeenCalledOnce();
  expect(vi.mocked(client.start).mock.calls[0]?.[0]).toEqual(accessTarget);
});

it('checks the current account before restoring a saved handoff', async () => {
  const { client } = fixture();
  sessionStorage.setItem(
    `cma.connection.${JSON.stringify(target)}`,
    JSON.stringify({
      owner: 'other-user',
      service_id: subscriptionService,
      intent_id: 'old-intent',
      hosted_url: 'https://auth.openai.com/codex/device',
      expires_at: new Date(Date.now() + 60000).toISOString(),
    }),
  );
  render(<ConnectionWizard target={target} client={client} onReady={vi.fn()} />);
  await flush();
  await poll();
  expect(vi.mocked(client.check).mock.calls.every((call) => call[1] === undefined)).toBe(true);
  expect(sessionStorage.getItem(`cma.connection.${JSON.stringify(target)}`)).toBeNull();
  expect(client.start).not.toHaveBeenCalled();
});

it('restores an owner-matched pending login without issuing another start', async () => {
  const { client } = fixture();
  const active = {
    owner: 'owner',
    service_id: subscriptionService,
    intent_id: 'saved-intent',
    hosted_url: 'https://auth.openai.com/codex/device',
    expires_at: new Date(Date.now() + 60000).toISOString(),
    user_code: 'ABCD-1234',
  };
  sessionStorage.setItem(`cma.connection.${JSON.stringify(target)}`, JSON.stringify(active));
  render(<ConnectionWizard target={target} client={client} onReady={vi.fn()} />);
  await flush();
  await poll();
  expect(vi.mocked(client.check).mock.calls[0]?.[1]).toBeUndefined();
  expect(vi.mocked(client.check).mock.calls[1]?.[1]).toEqual(active);
  expect(screen.getByText('ABCD-1234')).toBeInTheDocument();
  expect(client.start).not.toHaveBeenCalled();
});

it('cancels a late start response without navigating or continuing', async () => {
  const { client } = fixture();
  let finish!: (value: StartedConnection) => void;
  vi.mocked(client.start).mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const ready = vi.fn();
  const cancel = vi.fn();
  render(<ConnectionWizard target={target} client={client} onReady={ready} onCancel={cancel} />);
  await flush();
  fireEvent.click(screen.getByRole('button', { name: 'Connect all' }));
  fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  const started = {
    ...vi.mocked(client.start).mock.calls[0]![1],
    owner: 'owner',
    hosted_url: 'https://identity.example.test/connect',
    expires_at: new Date(Date.now() + 60000).toISOString(),
  };
  await act(async () => finish(started));
  expect(client.cancel).toHaveBeenCalledWith(target, started);
  expect(cancel).toHaveBeenCalledOnce();
  expect(ready).not.toHaveBeenCalled();
  expect(popup.location.href).toBe('');
});

it('does not mistake an older usable subscription for completed forced re-login', async () => {
  const { client, connected, report } = fixture();
  services.forEach((service) => connected.add(service));
  vi.mocked(client.check).mockImplementation(async (_target, active) =>
    active === undefined
      ? report()
      : {
          ...report(),
          ready: false,
          active_state: 'pending',
          services: report().services.map((row) =>
            row.service === subscriptionService ? { ...row, state: 'missing' } : row,
          ),
        },
  );
  const ready = vi.fn();
  render(<ConnectionWizard target={target} client={client} forceSubscription onReady={ready} />);
  await flush();
  expect(ready).not.toHaveBeenCalled();
  fireEvent.click(
    screen.getByRole('button', { name: 'Sign in to your ChatGPT subscription again' }),
  );
  await flush();
  await poll();
  expect(client.start).toHaveBeenCalledWith(
    target,
    expect.objectContaining({ service_id: subscriptionService }),
    true,
  );
  expect(ready).not.toHaveBeenCalled();
  vi.mocked(client.check).mockResolvedValue({ ...report(), active_state: 'satisfied' });
  await poll();
  expect(ready).toHaveBeenCalledOnce();
});

it('fences pending starts when the selected sandbox changes', async () => {
  const { client } = fixture();
  let finish!: (value: StartedConnection) => void;
  vi.mocked(client.start).mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const ready = vi.fn();
  const view = render(<ConnectionWizard target={target} client={client} onReady={ready} />);
  await flush();
  fireEvent.click(screen.getByRole('button', { name: 'Connect all' }));
  view.rerender(
    <ConnectionWizard
      target={{ type: 'sandbox', id: 'different' }}
      client={client}
      onReady={ready}
    />,
  );
  await flush();
  const started = {
    ...vi.mocked(client.start).mock.calls[0]![1],
    owner: 'owner',
    hosted_url: 'https://identity.example.test/connect',
    expires_at: new Date(Date.now() + 60000).toISOString(),
  };
  await act(async () => finish(started));
  expect(client.cancel).toHaveBeenCalledWith(target, started);
  expect(ready).not.toHaveBeenCalled();
  expect(screen.queryByText(/Waiting for sign-in/)).not.toBeInTheDocument();
});

it('names the subscription row and group without the agent provider', async () => {
  const { client } = fixture();
  render(<ConnectionWizard target={target} client={client} onReady={vi.fn()} />);
  await flush();
  expect(screen.getByRole('heading', { name: 'Model subscription' })).toBeInTheDocument();
  expect(screen.getByText('Your ChatGPT subscription')).toBeInTheDocument();
  // Other services keep the name their record gives them.
  expect(screen.getByText('GitHub')).toBeInTheDocument();
  expect(document.body.textContent).not.toMatch(/codex/i);
});
