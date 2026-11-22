import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ConnectionWizard } from './ConnectionWizard';
import type { ConnectionClient, ConnectionReport, ConnectionTarget } from './client';

const target: ConnectionTarget = { type: 'launch', id: 'original-launch' };
const flush = () => act(async () => undefined);
const poll = () => act(async () => vi.advanceTimersByTimeAsync(2000));
const fallback = () => screen.queryByRole('link', { name: 'Continue in NyxID' });

beforeEach(() => {
  vi.useFakeTimers();
  vi.stubEnv('VITE_CONNECTION_INTENT_RECOVERY_ENABLED', 'true');
  sessionStorage.clear();
  window.history.replaceState(null, '', '/launches/original-launch');
  vi.spyOn(window, 'open').mockReturnValue(null);
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllEnvs();
  vi.restoreAllMocks();
  sessionStorage.clear();
  window.history.replaceState(null, '', '/');
});

function fixture() {
  const report: ConnectionReport = {
    owner: 'original-owner',
    ready: false,
    checked_at: new Date().toISOString(),
    services: [
      {
        service: 'code',
        display_name: 'Code host',
        kind: 'nyxid',
        state: 'missing',
        connection_mode: 'hosted',
      },
    ],
  };
  const client: ConnectionClient = {
    check: vi.fn(async () => report),
    start: vi.fn(async (_target, active) => ({
      ...active,
      owner: report.owner,
      hosted_url: 'https://identity.example.test/connect/original-intent',
      expires_at: new Date(Date.now() + 600000).toISOString(),
    })),
    cancel: vi.fn(async () => undefined),
  };
  return { client, report };
}

async function start(client: ConnectionClient, onReady = vi.fn()) {
  const view = render(<ConnectionWizard target={target} client={client} onReady={onReady} />);
  await flush();
  fireEvent.click(screen.getByRole('button', { name: 'Connect all' }));
  await flush();
  return view;
}

it('offers the acknowledged URL in the same tab without starting a second intent', async () => {
  const { client, report } = fixture();
  const ready = vi.fn();
  const view = await start(client, ready);
  expect(fallback()).toHaveAttribute(
    'href',
    'https://identity.example.test/connect/original-intent',
  );
  expect(fallback()).not.toHaveAttribute('target');
  expect(screen.getByText(/Continue in this tab, then return here/)).toBeInTheDocument();
  expect(client.start).toHaveBeenCalledOnce();
  expect(ready).not.toHaveBeenCalled();
  const original = vi.mocked(client.start).mock.calls[0]![1];
  // Returning to the original route must verify the saved intent before continuing.
  view.unmount();
  vi.mocked(client.check).mockResolvedValue({
    ...report,
    ready: true,
    active_state: 'satisfied',
    services: report.services.map((row) => ({ ...row, state: 'satisfied' })),
  });
  render(<ConnectionWizard target={target} client={client} onReady={ready} />);
  await flush();
  await poll();
  await poll();
  expect(client.check).toHaveBeenCalledWith(
    target,
    expect.objectContaining(original),
    expect.any(AbortSignal),
  );
  expect(ready).toHaveBeenCalledOnce();
  expect(client.start).toHaveBeenCalledOnce();
  expect(client.cancel).not.toHaveBeenCalled();
});

it.each([undefined, 'false', 'TRUE'])('keeps the fallback off for flag %s', async (value) => {
  vi.stubEnv('VITE_CONNECTION_INTENT_RECOVERY_ENABLED', value);
  await start(fixture().client);
  expect(fallback()).not.toBeInTheDocument();
});

it('keeps the existing popup path when a window is available', async () => {
  const popup = {
    opener: {},
    closed: false,
    location: { href: '' },
    focus: vi.fn(),
    close: vi.fn(),
  };
  vi.mocked(window.open).mockReturnValue(popup as unknown as Window);
  await start(fixture().client);
  expect(popup.location.href).toBe('https://identity.example.test/connect/original-intent');
  expect(fallback()).not.toBeInTheDocument();
});

it.each(['expired', 'failed', 'cancelled'] as const)(
  'removes the link after a %s response',
  async (active_state) => {
    const { client, report } = fixture();
    await start(client);
    expect(fallback()).toBeInTheDocument();
    vi.mocked(client.check).mockResolvedValue({ ...report, active_state });
    await poll();
    expect(fallback()).not.toBeInTheDocument();
  },
);

it('removes the link when the owner changes or cancellation is requested', async () => {
  const { client, report } = fixture();
  await start(client);
  expect(fallback()).toBeInTheDocument();
  vi.mocked(client.check).mockResolvedValue({ ...report, owner: 'other-owner' });
  await poll();
  expect(fallback()).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  expect(fallback()).not.toBeInTheDocument();
  expect(client.cancel).toHaveBeenCalledOnce();
});

it.each(['owner', 'intent_id', 'hosted_url'] as const)(
  'refuses an invalid %s start reply',
  async (field) => {
    const { client } = fixture();
    vi.mocked(client.start).mockImplementation(async (_target, active) => ({
      ...active,
      owner: 'original-owner',
      hosted_url: 'https://identity.example.test/connect/original-intent',
      expires_at: new Date(Date.now() + 600000).toISOString(),
      [field]: field === 'hosted_url' ? 'javascript:alert(1)' : 'wrong',
    }));
    await start(client);
    expect(screen.getByRole('alert')).toBeInTheDocument();
    expect(fallback()).not.toBeInTheDocument();
  },
);

it('saves the return route only on explicit fallback navigation and preserves the intent', async () => {
  const { client } = fixture();
  await start(client);
  const recoveryKey = `cma.connection-recovery.v1.${JSON.stringify(target)}:false`;
  const original = sessionStorage.getItem(recoveryKey);
  expect(sessionStorage.getItem('cma.connection-same-tab.v1')).toBeNull();
  const link = fallback()!;
  link.addEventListener('click', (event) => event.preventDefault());
  fireEvent.click(link);
  expect(JSON.parse(sessionStorage.getItem('cma.connection-same-tab.v1')!)).toMatchObject({
    path: '/launches/original-launch',
  });
  expect(sessionStorage.getItem(recoveryKey)).toBe(original);
  expect(client.start).toHaveBeenCalledOnce();
  expect(client.cancel).not.toHaveBeenCalled();
});

it('blocks fallback navigation if storing the return route fails', async () => {
  const { client } = fixture();
  await start(client);
  vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
    throw new Error('storage unavailable');
  });
  const event = new MouseEvent('click', { bubbles: true, cancelable: true });
  fireEvent(fallback()!, event);
  expect(event.defaultPrevented).toBe(true);
  expect(screen.getByRole('alert')).toHaveTextContent('Your return to CMA could not be saved');
  expect(client.start).toHaveBeenCalledOnce();
  expect(client.cancel).not.toHaveBeenCalled();
});

it('keeps actions held only in memory on their existing popup path', async () => {
  const { client } = fixture();
  render(
    <ConnectionWizard
      target={{ type: 'agent', id: 'original-agent' }}
      client={client}
      onReady={vi.fn()}
    />,
  );
  await flush();
  fireEvent.click(screen.getByRole('button', { name: 'Connect all' }));
  await flush();
  expect(fallback()).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Open sign-in window' })).toBeInTheDocument();
});
