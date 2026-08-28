import { afterEach, describe, expect, it, vi } from 'vitest';
import { ApiError, TransportError } from '../api/client';
import {
  ConnectionAuthorizationCancelled,
  ConnectionAuthorizationTimedOut,
  ConnectionPopupBlocked,
  INACTIVE_TRIGGER_CONNECTION_GUIDANCE,
  authorizeTriggerConnection,
  isActiveTriggerConnection,
  isReconnectableTriggerConnection,
} from './connectionFlow';
import type { ConnectionAuthorization, TriggerConnection } from '@chronoai/cma-sdk';

const HANDOFF: ConnectionAuthorization = {
  service_id: 'service-exact',
  authorization_url:
    'https://github.com/login/oauth/authorize?client_id=fixture&state=12345678-1234-4234-8234-123456789abc',
  attempt_nonce: '12345678-1234-4234-8234-123456789abc',
};

const ACTIVE: TriggerConnection = {
  service_id: 'service-exact',
  state_version: 2,
  state: 'active',
  account_id: 42,
  account_node_id: 'U_fixture',
  account_login: 'contributor',
  avatar_url: null,
  granted_scopes: ['public_repo'],
  consent_observed_at: '2026-08-27T00:00:00Z',
};

afterEach(() => {
  vi.useRealTimers();
});

function popup() {
  return {
    closed: false,
    location: { href: 'about:blank' },
    close: vi.fn(function (this: { closed: boolean }) {
      this.closed = true;
    }),
  };
}

describe('GitHub connection authorization', () => {
  it('opens synchronously before waiting for the CMA handoff', async () => {
    let resolve!: (value: ConnectionAuthorization) => void;
    const begin = vi.fn(
      () =>
        new Promise<ConnectionAuthorization>((done) => {
          resolve = done;
        }),
    );
    const child = popup();
    const open = vi.fn(() => child as unknown as Window);

    const pending = authorizeTriggerConnection({
      begin,
      preflight: vi.fn().mockResolvedValue(ACTIVE),
      open: open as typeof window.open,
    });

    expect(open).toHaveBeenCalledOnce();
    expect(child.location.href).toBe('about:blank');
    resolve(HANDOFF);
    await expect(pending).resolves.toEqual(ACTIVE);
    expect(child.location.href).toBe(HANDOFF.authorization_url);
    expect(child.close).toHaveBeenCalledOnce();
  });

  it('fails before requesting a handoff when the popup is blocked', async () => {
    const begin = vi.fn();
    await expect(
      authorizeTriggerConnection({
        begin,
        preflight: vi.fn(),
        open: vi.fn(() => null),
      }),
    ).rejects.toBeInstanceOf(ConnectionPopupBlocked);
    expect(begin).not.toHaveBeenCalled();
  });

  it.each([
    'http://github.com/login/oauth/authorize',
    'https://github.example.test/login/oauth/authorize',
    'https://github.com/settings/applications',
    'https://user@github.com/login/oauth/authorize',
    'https://github.com/login/oauth/authorize#fragment',
  ])('closes the blank popup for an unexpected destination: %s', async (authorizationUrl) => {
    const child = popup();
    await expect(
      authorizeTriggerConnection({
        begin: vi.fn().mockResolvedValue({ ...HANDOFF, authorization_url: authorizationUrl }),
        preflight: vi.fn(),
        open: vi.fn(() => child as unknown as Window),
      }),
    ).rejects.toThrow(/authorization/i);
    expect(child.close).toHaveBeenCalledOnce();
  });

  it('polls only the exact service while OAuth is still pending', async () => {
    const child = popup();
    const preflight = vi
      .fn()
      .mockRejectedValueOnce(
        new ApiError(409, 'connection inactive', 'trigger_connection_inactive'),
      )
      .mockResolvedValueOnce(ACTIVE);
    const onService = vi.fn();

    await expect(
      authorizeTriggerConnection({
        begin: vi.fn().mockResolvedValue(HANDOFF),
        preflight,
        onService,
        inactiveIsPending: true,
        pollMs: 1,
        open: vi.fn(() => child as unknown as Window),
      }),
    ).resolves.toEqual(ACTIVE);

    expect(onService).toHaveBeenCalledWith('service-exact');
    expect(preflight).toHaveBeenNthCalledWith(1, 'service-exact', expect.any(AbortSignal));
    expect(preflight).toHaveBeenNthCalledWith(2, 'service-exact', expect.any(AbortSignal));
  });

  it('does not treat an inactive existing service as reconnectable OAuth progress', async () => {
    const child = popup();
    await expect(
      authorizeTriggerConnection({
        begin: vi.fn().mockResolvedValue(HANDOFF),
        preflight: vi
          .fn()
          .mockRejectedValue(
            new ApiError(409, 'connection inactive', 'trigger_connection_inactive'),
          ),
        pollMs: 1,
        open: vi.fn(() => child as unknown as Window),
      }),
    ).rejects.toMatchObject({ code: 'trigger_connection_inactive' });
    expect(child.close).toHaveBeenCalledOnce();
  });

  it('refuses a preflight response for a different service', async () => {
    const child = popup();
    await expect(
      authorizeTriggerConnection({
        begin: vi.fn().mockResolvedValue(HANDOFF),
        preflight: vi.fn().mockResolvedValue({ ...ACTIVE, service_id: 'service-other' }),
        open: vi.fn(() => child as unknown as Window),
      }),
    ).rejects.toThrow(/different GitHub connection/i);
    expect(child.close).toHaveBeenCalledOnce();
  });

  it('honors a long Retry-After bound before polling again', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(0);
    const preflight = vi
      .fn()
      .mockRejectedValueOnce(new ApiError(429, 'slow down', 'rate_limited', undefined, 30_000))
      .mockResolvedValueOnce(ACTIVE);
    const authorization = authorizeTriggerConnection({
      begin: vi.fn().mockResolvedValue(HANDOFF),
      preflight,
      pollMs: 1_000,
      timeoutMs: 60_000,
      open: vi.fn(() => popup() as unknown as Window),
    });
    await vi.advanceTimersByTimeAsync(0);
    expect(preflight).toHaveBeenCalledOnce();

    await vi.advanceTimersByTimeAsync(29_999);
    expect(preflight).toHaveBeenCalledOnce();
    await vi.advanceTimersByTimeAsync(1);

    await expect(authorization).resolves.toEqual(ACTIVE);
    expect(preflight).toHaveBeenCalledTimes(2);
  });

  it('backs off increasing transient failures', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(0);
    const calledAt: number[] = [];
    const preflight = vi
      .fn()
      .mockImplementationOnce(async () => {
        calledAt.push(Date.now());
        throw new TransportError('offline');
      })
      .mockImplementationOnce(async () => {
        calledAt.push(Date.now());
        throw new ApiError(503, 'unavailable');
      })
      .mockImplementationOnce(async () => {
        calledAt.push(Date.now());
        return ACTIVE;
      });
    const authorization = authorizeTriggerConnection({
      begin: vi.fn().mockResolvedValue(HANDOFF),
      preflight,
      pollMs: 1_000,
      timeoutMs: 10_000,
      open: vi.fn(() => popup() as unknown as Window),
    });

    await vi.advanceTimersByTimeAsync(1_000);
    expect(preflight).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(1_999);
    expect(preflight).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(1);

    await expect(authorization).resolves.toEqual(ACTIVE);
    expect(calledAt).toEqual([0, 1_000, 3_000]);
  });

  it('resets transient backoff after a successful pending poll', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(0);
    const pendingConnection = { ...ACTIVE, state: 'disconnected' as const, account_id: null };
    const calledAt: number[] = [];
    const outcomes: Array<TriggerConnection | Error> = [
      new TransportError('offline'),
      pendingConnection,
      new TransportError('offline again'),
      ACTIVE,
    ];
    const preflight = vi.fn(async () => {
      calledAt.push(Date.now());
      const outcome = outcomes.shift();
      if (outcome instanceof Error) throw outcome;
      return outcome!;
    });
    const authorization = authorizeTriggerConnection({
      begin: vi.fn().mockResolvedValue(HANDOFF),
      preflight,
      pollMs: 1_000,
      timeoutMs: 10_000,
      open: vi.fn(() => popup() as unknown as Window),
    });

    await vi.advanceTimersByTimeAsync(3_000);

    await expect(authorization).resolves.toEqual(ACTIVE);
    expect(calledAt).toEqual([0, 1_000, 2_000, 3_000]);
  });

  it('cancels while waiting on a transient retry bound', async () => {
    vi.useFakeTimers();
    const controller = new AbortController();
    const authorization = authorizeTriggerConnection({
      begin: vi.fn().mockResolvedValue(HANDOFF),
      preflight: vi
        .fn()
        .mockRejectedValue(new ApiError(429, 'slow down', 'rate_limited', undefined, 60_000)),
      signal: controller.signal,
      pollMs: 1_000,
      open: vi.fn(() => popup() as unknown as Window),
    });
    await vi.advanceTimersByTimeAsync(0);

    controller.abort();

    await expect(authorization).rejects.toBeInstanceOf(ConnectionAuthorizationCancelled);
  });

  it('keeps the overall authorization deadline authoritative', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(0);
    const authorization = authorizeTriggerConnection({
      begin: vi.fn().mockResolvedValue(HANDOFF),
      preflight: vi
        .fn()
        .mockRejectedValue(new ApiError(429, 'slow down', 'rate_limited', undefined, 60_000)),
      pollMs: 1_000,
      timeoutMs: 5_000,
      open: vi.fn(() => popup() as unknown as Window),
    });
    const outcome = expect(authorization).rejects.toBeInstanceOf(ConnectionAuthorizationTimedOut);
    await vi.advanceTimersByTimeAsync(5_000);

    await outcome;
  });
});

describe('active connection classification', () => {
  it('requires both the active state and a numeric account', () => {
    expect(isActiveTriggerConnection(ACTIVE)).toBe(true);
    expect(isActiveTriggerConnection({ ...ACTIVE, account_id: null })).toBe(false);
    expect(isActiveTriggerConnection({ ...ACTIVE, state: 'inactive' })).toBe(false);
    expect(isActiveTriggerConnection(null)).toBe(false);
  });

  it('allows reconnect only for a disconnected or authorization-empty active service', () => {
    expect(isReconnectableTriggerConnection({ ...ACTIVE, state: 'disconnected' })).toBe(true);
    expect(isReconnectableTriggerConnection({ ...ACTIVE, account_id: null })).toBe(true);
    expect(isReconnectableTriggerConnection({ ...ACTIVE, state: 'inactive' })).toBe(false);
    expect(isReconnectableTriggerConnection({ ...ACTIVE, state: 'revoked' })).toBe(false);
    expect(isReconnectableTriggerConnection({ ...ACTIVE, state: 'expired' })).toBe(false);
    expect(INACTIVE_TRIGGER_CONNECTION_GUIDANCE).toMatch(/Enable this exact service in NyxID/);
  });
});
