import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ConnectionWizard } from './ConnectionWizard';
import {
  subscriptionService,
  type ConnectionClient,
  type ConnectionReport,
  type ConnectionTarget,
} from './client';

const target: ConnectionTarget = { type: 'launch', id: 'original-launch' };
const flush = () => act(async () => undefined);
const poll = () =>
  act(async () => {
    await vi.advanceTimersByTimeAsync(2000);
  });
const click = (name = 'Connect all') => fireEvent.click(screen.getByRole('button', { name }));

beforeEach(() => {
  vi.useFakeTimers();
  vi.stubEnv('VITE_CONNECTION_INTENT_RECOVERY_ENABLED', 'true');
  sessionStorage.clear();
  vi.spyOn(window, 'open').mockReturnValue(null);
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllEnvs();
  vi.restoreAllMocks();
  sessionStorage.clear();
});

function fixture(service = 'code') {
  let owner = 'owner';
  let ready = false;
  let state: ConnectionReport['active_state'];
  const client: ConnectionClient = {
    check: vi.fn(async (): Promise<ConnectionReport> => ({
      owner,
      ready,
      checked_at: new Date().toISOString(),
      active_state: state,
      services: [
        {
          service,
          display_name: 'Code host',
          kind: 'nyxid',
          state: ready ? 'satisfied' : 'missing',
          connection_mode: 'hosted',
        },
      ],
    })),
    start: vi.fn(async (_target, active) => ({
      ...active,
      owner,
      hosted_url: 'https://identity.example.test/connect',
      expires_at: new Date(Date.now() + 1000).toISOString(),
    })),
    cancel: vi.fn(async () => undefined),
  };
  return {
    client,
    connect: () => {
      ready = true;
    },
    state: (value: typeof state) => {
      state = value;
    },
    owner: (value: string) => {
      owner = value;
    },
  };
}

it.each(['expired', 'failed', 'cancelled'] as const)(
  'fences %s even when the same response reports all services ready',
  async (state) => {
    const f = fixture();
    const ready = vi.fn();
    render(
      <ConnectionWizard target={target} client={f.client} onReady={ready} presentation="dialog" />,
    );
    await flush();
    click();
    await flush();
    f.state(state);
    f.connect();
    await poll();
    expect(ready).not.toHaveBeenCalled();
    expect(screen.getByRole('dialog')).toBeInTheDocument();
    expect(screen.getByRole('alert')).toHaveTextContent('This sign-in did not finish');
    click('Verify again');
    await flush();
    await poll();
    expect(ready).not.toHaveBeenCalled();
    click('Continue');
    await poll();
    await poll();
    expect(ready).toHaveBeenCalledOnce();
    expect(f.client.start).toHaveBeenCalledOnce();
  },
);

it('honors server pending beyond the local deadline and completes without another click', async () => {
  const f = fixture();
  const ready = vi.fn();
  render(<ConnectionWizard target={target} client={f.client} onReady={ready} />);
  await flush();
  click();
  await flush();
  f.state('pending');
  await poll();
  await poll();
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(ready).not.toHaveBeenCalled();
  f.state('satisfied');
  f.connect();
  await poll();
  expect(ready).toHaveBeenCalledOnce();
});

it('restores an uncertain intent after remount but only replays it on an explicit click', async () => {
  const f = fixture();
  const ready = vi.fn();
  vi.mocked(f.client.start).mockRejectedValueOnce(new Error('Response lost'));
  const view = render(<ConnectionWizard target={target} client={f.client} onReady={ready} />);
  await flush();
  click();
  await flush();
  const original = vi.mocked(f.client.start).mock.calls[0];
  view.unmount();
  render(<ConnectionWizard target={target} client={f.client} onReady={ready} />);
  await flush();
  await poll();
  expect(f.client.start).toHaveBeenCalledOnce();
  expect(f.client.check).toHaveBeenLastCalledWith(
    target,
    expect.objectContaining(original![1]),
    expect.any(AbortSignal),
  );
  click();
  await flush();
  expect(vi.mocked(f.client.start).mock.calls[1]).toEqual(original);
  expect(ready).not.toHaveBeenCalled();
});

it('requires confirmation when an uncertain request completed before remount', async () => {
  const f = fixture();
  const ready = vi.fn();
  vi.mocked(f.client.start).mockRejectedValueOnce(new Error('Response lost'));
  const view = render(<ConnectionWizard target={target} client={f.client} onReady={ready} />);
  await flush();
  click();
  await flush();
  view.unmount();
  f.connect();
  f.state('satisfied');
  render(<ConnectionWizard target={target} client={f.client} onReady={ready} />);
  await flush();
  await poll();
  expect(ready).not.toHaveBeenCalled();
  click('Continue');
  await poll();
  expect(ready).toHaveBeenCalledOnce();
  expect(f.client.start).toHaveBeenCalledOnce();
});

it('does not replay an old owner intent after remount', async () => {
  const f = fixture();
  vi.mocked(f.client.start).mockRejectedValueOnce(new Error('Response lost'));
  const view = render(<ConnectionWizard target={target} client={f.client} onReady={vi.fn()} />);
  await flush();
  click();
  await flush();
  const previous = vi.mocked(f.client.start).mock.calls[0]![1];
  view.unmount();
  f.owner('second-owner');
  vi.mocked(f.client.check).mockClear();
  render(<ConnectionWizard target={target} client={f.client} onReady={vi.fn()} />);
  await flush();
  await poll();
  expect(vi.mocked(f.client.check).mock.calls.every((call) => call[1] === undefined)).toBe(true);
  click();
  await flush();
  expect(vi.mocked(f.client.start).mock.calls[1]![1].intent_id).not.toBe(previous.intent_id);
});

it('keeps uncertain intents isolated by target', async () => {
  const f = fixture();
  vi.mocked(f.client.start).mockRejectedValueOnce(new Error('Response lost'));
  const view = render(<ConnectionWizard target={target} client={f.client} onReady={vi.fn()} />);
  await flush();
  click();
  await flush();
  const original = vi.mocked(f.client.start).mock.calls[0];
  view.rerender(
    <ConnectionWizard
      target={{ type: 'launch', id: 'second-launch' }}
      client={f.client}
      onReady={vi.fn()}
    />,
  );
  await flush();
  click();
  await flush();
  expect(vi.mocked(f.client.start).mock.calls[1]![1].intent_id).not.toBe(original![1].intent_id);
  view.rerender(<ConnectionWizard target={target} client={f.client} onReady={vi.fn()} />);
  await flush();
  await poll();
  click();
  await flush();
  expect(vi.mocked(f.client.start).mock.calls[2]).toEqual(original);
});

it('preserves the forced subscription payload after a lost response', async () => {
  const f = fixture(subscriptionService);
  f.connect();
  vi.mocked(f.client.start).mockRejectedValueOnce(new Error('Response lost'));
  const view = render(
    <ConnectionWizard target={target} client={f.client} forceSubscription onReady={vi.fn()} />,
  );
  await flush();
  click('Sign in to your ChatGPT subscription again');
  await flush();
  const original = vi.mocked(f.client.start).mock.calls[0];
  expect(original![2]).toBe(true);
  view.unmount();
  f.state('pending');
  render(
    <ConnectionWizard target={target} client={f.client} forceSubscription onReady={vi.fn()} />,
  );
  await flush();
  await poll();
  click('Sign in to your ChatGPT subscription again');
  await flush();
  expect(vi.mocked(f.client.start).mock.calls[1]).toEqual(original);
});

it('refuses a new start when its intent cannot be stored', async () => {
  const f = fixture();
  render(<ConnectionWizard target={target} client={f.client} onReady={vi.fn()} />);
  await flush();
  vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
    throw new Error('Storage unavailable');
  });
  click();
  await flush();
  expect(f.client.start).not.toHaveBeenCalled();
  expect(screen.getByRole('alert')).toHaveTextContent('Storage unavailable');
});

it('cancels an uncertain intent and keeps the original continuation fenced after remount', async () => {
  const f = fixture();
  const ready = vi.fn();
  vi.mocked(f.client.start).mockRejectedValueOnce(new Error('Response lost'));
  const view = render(
    <ConnectionWizard target={target} client={f.client} onReady={ready} onCancel={vi.fn()} />,
  );
  await flush();
  click();
  await flush();
  click('Cancel');
  await flush();
  expect(f.client.cancel).toHaveBeenCalledWith(
    target,
    expect.objectContaining(vi.mocked(f.client.start).mock.calls[0]![1]),
  );
  view.unmount();
  f.connect();
  render(<ConnectionWizard target={target} client={f.client} onReady={ready} />);
  await flush();
  await poll();
  expect(ready).not.toHaveBeenCalled();
});

it.each([undefined, 'false', 'TRUE'])('preserves the legacy path with flag %s', async (flag) => {
  vi.stubEnv('VITE_CONNECTION_INTENT_RECOVERY_ENABLED', flag);
  const f = fixture();
  const ready = vi.fn();
  render(<ConnectionWizard target={target} client={f.client} onReady={ready} />);
  await flush();
  click();
  await flush();
  await poll();
  f.connect();
  await poll();
  expect(ready).toHaveBeenCalledOnce();
  expect(
    Object.keys(sessionStorage).some((key) => key.startsWith('cma.connection-recovery.')),
  ).toBe(false);
});

it('refuses malformed saved display data before restoring a handoff', async () => {
  const f = fixture();
  const ready = vi.fn();
  sessionStorage.setItem(
    `cma.connection-recovery.v1.${JSON.stringify(target)}:false`,
    JSON.stringify({
      owner: 'owner',
      needsResume: false,
      pending: {
        service_id: 'code',
        intent_id: 'retained-intent',
        force: false,
        started: {
          owner: 'owner',
          service_id: 'code',
          intent_id: 'retained-intent',
          hosted_url: 'https://identity.example.test/connect',
          expires_at: new Date(Date.now() + 60000).toISOString(),
          user_code: { invalid: true },
        },
      },
    }),
  );
  render(<ConnectionWizard target={target} client={f.client} onReady={ready} />);
  await flush();
  await poll();
  expect(f.client.start).not.toHaveBeenCalled();
  expect(vi.mocked(f.client.check).mock.calls.every((call) => call[1] === undefined)).toBe(true);
  f.connect();
  await poll();
  expect(ready).not.toHaveBeenCalled();
  expect(screen.getByRole('button', { name: 'Continue' })).toBeInTheDocument();
});
