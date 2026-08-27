import { describe, expect, it, vi } from 'vitest';
import { ApiError } from '../api/client';
import {
  ConnectionPopupBlocked,
  authorizeTriggerConnection,
  isActiveTriggerConnection,
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
        pollMs: 1,
        open: vi.fn(() => child as unknown as Window),
      }),
    ).resolves.toEqual(ACTIVE);

    expect(onService).toHaveBeenCalledWith('service-exact');
    expect(preflight).toHaveBeenNthCalledWith(1, 'service-exact', expect.any(AbortSignal));
    expect(preflight).toHaveBeenNthCalledWith(2, 'service-exact', expect.any(AbortSignal));
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
});

describe('active connection classification', () => {
  it('requires both the active state and a numeric account', () => {
    expect(isActiveTriggerConnection(ACTIVE)).toBe(true);
    expect(isActiveTriggerConnection({ ...ACTIVE, account_id: null })).toBe(false);
    expect(isActiveTriggerConnection({ ...ACTIVE, state: 'inactive' })).toBe(false);
    expect(isActiveTriggerConnection(null)).toBe(false);
  });
});
