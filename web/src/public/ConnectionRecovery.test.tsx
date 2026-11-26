import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ServiceConnectionStatus } from '@cma/client';
import { PUBLIC_RECORD, PUBLIC_TRUST, publicClient } from '../test/public';
import { ConnectAssistant } from './ConnectAssistant';
import { rememberConnectionReturn } from './connectionReturn';
import { ReviewCard } from './ReviewCard';
import { connectionClient } from '../connections/client';

const service = {
  service: 'code',
  state: 'missing',
  display_name: 'Code host',
  connection_mode: 'hosted',
} as const;
const pending = {
  handoff_id: 'handoff-one',
  launch_id: PUBLIC_RECORD.id,
  service_id: service.service,
  state: 'pending',
  expires_at: '2026-09-06T00:10:00Z',
  updated_at: PUBLIC_RECORD.updated_at,
} as const;

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date('2026-09-06T00:00:00Z'));
});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  sessionStorage.clear();
});

function setup() {
  const client = publicClient();
  const satisfied = vi.fn();
  const recheck = vi.fn();
  render(
    <ConnectAssistant
      client={client}
      requestId={PUBLIC_RECORD.id}
      service={service}
      onSatisfied={satisfied}
      onRecheck={recheck}
    />,
  );
  return { client, satisfied, recheck };
}

describe('pending connection recovery', () => {
  it('reopens the same unexpired link without creating another handoff', async () => {
    const popup = { opener: {}, location: { href: '' }, close: vi.fn(), closed: false };
    const open = vi.spyOn(window, 'open').mockReturnValue(popup as unknown as Window);
    const { client, satisfied } = setup();
    fireEvent.click(screen.getByRole('button', { name: 'Connect' }));
    await act(async () => undefined);
    popup.closed = true;
    fireEvent.click(screen.getByRole('button', { name: 'Connect' }));
    await act(async () => undefined);
    expect(open).toHaveBeenCalledTimes(2);
    expect(client.requests.createConnection).toHaveBeenCalledOnce();
    expect(popup.location.href).toBe('https://identity.example.test/connect');
    expect(satisfied).not.toHaveBeenCalled();
  });

  it('verifies immediately, reports missing access, and can retry to completion', async () => {
    vi.spyOn(window, 'open').mockReturnValue(null);
    const { client, satisfied } = setup();
    vi.mocked(client.requests.readConnection).mockResolvedValue(pending);
    fireEvent.click(screen.getByRole('button', { name: 'Connect' }));
    await act(async () => undefined);
    fireEvent.click(screen.getByRole('button', { name: 'Verify' }));
    await act(async () => undefined);
    expect(client.requests.readConnection).toHaveBeenCalledOnce();
    expect(screen.getByText(/This account is not connected yet/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Connect' })).toBeEnabled();
    expect(satisfied).not.toHaveBeenCalled();
    vi.mocked(client.requests.readConnection).mockResolvedValue({ ...pending, state: 'satisfied' });
    fireEvent.click(screen.getByRole('button', { name: 'Verify' }));
    await act(async () => undefined);
    expect(satisfied).toHaveBeenCalledExactlyOnceWith('code');
    expect(screen.getByText(/This account is not connected yet/)).toHaveAttribute('inert');
    await act(() => vi.advanceTimersByTimeAsync(200));
    expect(screen.queryByText(/This account is not connected yet/)).not.toBeInTheDocument();
  });

  it('keeps controls disabled while an explicit verification is in flight', async () => {
    vi.spyOn(window, 'open').mockReturnValue(null);
    const { client, satisfied } = setup();
    let settle!: (value: ServiceConnectionStatus) => void;
    vi.mocked(client.requests.readConnection).mockImplementation(
      () =>
        new Promise((resolve) => {
          settle = resolve;
        }),
    );
    fireEvent.click(screen.getByRole('button', { name: 'Connect' }));
    await act(async () => undefined);
    fireEvent.click(screen.getByRole('button', { name: 'Verify' }));
    expect(screen.getByRole('button', { name: 'Verifying' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Connect' })).toBeDisabled();
    await act(async () => settle(pending));
    expect(screen.getByRole('button', { name: 'Verify' })).toBeEnabled();
    expect(satisfied).not.toHaveBeenCalled();
  });

  it('refuses a satisfied result belonging to another handoff', async () => {
    vi.spyOn(window, 'open').mockReturnValue(null);
    const { client, satisfied } = setup();
    vi.mocked(client.requests.readConnection).mockResolvedValue({
      ...pending,
      handoff_id: 'different-handoff',
      state: 'satisfied',
    });
    fireEvent.click(screen.getByRole('button', { name: 'Connect' }));
    await act(async () => undefined);
    fireEvent.click(screen.getByRole('button', { name: 'Verify' }));
    await act(async () => undefined);
    expect(satisfied).not.toHaveBeenCalled();
    expect(screen.getByRole('alert')).toBeInTheDocument();
  });

  it('does not accept the old poll after explicitly requesting a replacement link', async () => {
    rememberConnectionReturn(PUBLIC_RECORD.id, service.service, pending);
    vi.spyOn(window, 'open').mockReturnValue(null);
    const { client, satisfied } = setup();
    let settle!: (value: ServiceConnectionStatus) => void;
    vi.mocked(client.requests.readConnection).mockImplementation(
      () =>
        new Promise((resolve) => {
          settle = resolve;
        }),
    );
    vi.mocked(client.requests.createConnection).mockImplementation(
      () => new Promise(() => undefined),
    );
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1500);
    });
    const signal = vi.mocked(client.requests.readConnection).mock.calls[0]?.[1]?.signal;
    fireEvent.click(screen.getByRole('button', { name: 'Connect' }));
    expect(signal?.aborted).toBe(true);
    await act(async () => settle({ ...pending, state: 'satisfied' }));
    expect(satisfied).not.toHaveBeenCalled();
    expect(screen.getByRole('button', { name: 'Connect' })).toBeDisabled();
  });

  it('offers a direct requirement check before any popup is needed', () => {
    const { client, recheck } = setup();
    fireEvent.click(screen.getByRole('button', { name: 'Verify' }));
    expect(recheck).toHaveBeenCalledOnce();
    expect(client.requests.createConnection).not.toHaveBeenCalled();
  });
});

it.each([false, true])(
  'continues once after three connections with prior landing consent: %s',
  async (landingConfirmed) => {
    const client = publicClient();
    const services = ['chrono-llm', 'ornn-api', 'api-github'];
    const record = {
      ...(landingConfirmed ? PUBLIC_TRUST : PUBLIC_RECORD),
      state: 'draft' as const,
      confirmations: landingConfirmed
        ? [
            {
              surface: 'landing' as const,
              trust_confirmed_digest: PUBLIC_TRUST.trust.confirmation_digest,
              acknowledgements: [],
              confirmed_at: PUBLIC_RECORD.created_at,
            },
          ]
        : [],
      review: {
        can_start_now: false,
        needs: services.map((service) => ({ type: 'connection' as const, service })),
      },
    };
    const connected = new Set<string>();
    vi.spyOn(connectionClient, 'check').mockImplementation(async () => ({
      owner: 'owner',
      ready: connected.size === services.length,
      checked_at: record.created_at,
      services: services.map((service) => ({
        service,
        display_name: service,
        kind: service === 'chrono-llm' ? 'codex' : 'nyxid',
        state: connected.has(service) ? 'satisfied' : 'missing',
        connection_mode: 'hosted',
      })),
    }));
    vi.spyOn(connectionClient, 'start').mockImplementation(async (_target, active) => ({
      ...active,
      owner: 'owner',
      hosted_url: 'https://identity.example.test/connect',
      expires_at: pending.expires_at,
    }));
    vi.spyOn(window, 'open').mockReturnValue(null);
    render(
      <ReviewCard
        expectedSubject="owner"
        client={client}
        record={record}
        onChanged={vi.fn()}
        onClose={vi.fn()}
      />,
    );
    await act(async () => undefined);
    expect(
      screen.getByRole('dialog', { name: 'NyxID Service Connection Verification' }),
    ).toBeInTheDocument();
    expect(screen.queryByRole('checkbox')).not.toBeInTheDocument();
    expect(screen.queryByRole('dialog', { name: 'Review and continue' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Verify connections' })).toBeDisabled();
    expect(client.requests.start).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Connect all' }));
    await act(async () => undefined);
    for (const service of services) {
      connected.add(service);
      await act(async () => {
        await vi.advanceTimersByTimeAsync(2000);
      });
    }
    expect(client.requests.start).toHaveBeenCalledOnce();
    expect(client.requests.start).toHaveBeenCalledWith(
      record.id,
      { trust_confirmed_digest: undefined, acknowledgements: [] },
      expect.any(Object),
    );
    expect(client.requests.createOrResumeBound).not.toHaveBeenCalled();
  },
);
