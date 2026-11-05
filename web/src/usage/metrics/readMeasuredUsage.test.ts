import { expect, it, vi } from 'vitest';
import { createClient } from '@chronoai/cma-sdk';
import { measuredUsageReader } from './readMeasuredUsage';

it.each([false, true, undefined])(
  'reads effective visibility %s through the public session SDK',
  async (visible) => {
    const fetch = vi.fn().mockResolvedValue(
      Response.json({
        authenticated: true,
        subject: 'owner-fixture',
        capabilities: ['use_own_sandboxes'],
        visitor: false,
        ...(visible === undefined ? {} : { usage_reporting: { measured_visible: visible } }),
      }),
    );
    const sdk = createClient({ baseUrl: 'http://localhost', fetch, cookies: true });
    const read = measuredUsageReader(sdk.auth);
    expect(await read({ signal: new AbortController().signal })).toBe(visible === true);
    expect(fetch).toHaveBeenCalledTimes(1);
    expect(String(fetch.mock.calls[0]![0])).toBe('http://localhost/auth/session');
    expect(fetch.mock.calls[0]![1]).toEqual(
      expect.objectContaining({ credentials: 'include', method: 'GET' }),
    );
  },
);
it('does not enable measured usage for an anonymous session', async () => {
  const sdk = createClient({
    baseUrl: 'http://localhost',
    fetch: vi.fn().mockResolvedValue(Response.json({ authenticated: false }, { status: 401 })),
  });
  expect(await measuredUsageReader(sdk.auth)({ signal: new AbortController().signal })).toBe(false);
});
it('propagates unavailable settings for the caller to fail closed', async () => {
  const sdk = createClient({
    baseUrl: 'http://localhost',
    fetch: vi.fn().mockResolvedValue(
      Response.json(
        {
          status: 503,
          code: 'settings_unavailable',
          title: 'Settings unavailable',
          detail: 'Retry later',
        },
        { status: 503 },
      ),
    ),
  });
  await expect(
    measuredUsageReader(sdk.auth)({ signal: new AbortController().signal }),
  ).rejects.toThrow();
});
