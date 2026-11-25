import { ApiError, ProtocolError } from '@cma/client';
import { afterEach, expect, it, vi } from 'vitest';
import { metricsClient } from './testFixtures';
import { clearDisabledViews, observeUsageView, viewUnavailable } from './viewCapability';

const signal = new AbortController().signal;

afterEach(() => vi.restoreAllMocks());

it('expires disabled observations and scopes them to the client, session and route', async () => {
  const api = metricsClient();
  const now = vi.spyOn(Date, 'now').mockReturnValue(10_000);
  const disabled = new ApiError(501, 'disabled', 'report_view_disabled');
  await expect(
    observeUsageView(api, 'one', signal, true, 'initial', () => Promise.reject(disabled)),
  ).rejects.toBe(disabled);
  expect(viewUnavailable(api, 'one', true, 'initial')).toBe(true);
  expect(viewUnavailable(api, 'two', true, 'initial')).toBe(false);
  expect(viewUnavailable(api, 'one', false, 'initial')).toBe(false);
  expect(viewUnavailable(api, 'one', true, 'latest')).toBe(false);
  expect(viewUnavailable(metricsClient(), 'one', true, 'initial')).toBe(false);
  now.mockReturnValue(40_000);
  expect(viewUnavailable(api, 'one', true, 'initial')).toBe(false);
  await expect(
    observeUsageView(api, 'one', signal, true, 'initial', () => Promise.reject(disabled)),
  ).rejects.toBe(disabled);
  clearDisabledViews(api, 'one');
  expect(viewUnavailable(api, 'one', true, 'initial')).toBe(false);
});

it.each([
  new ApiError(501, 'generic failure'),
  new ApiError(503, 'unavailable'),
  new ApiError(403, 'forbidden'),
  new ApiError(404, 'missing route'),
  new ProtocolError('invalid response'),
])(
  'keeps real failures and an old replica route miss outside the disabled cache',
  async (error) => {
    const api = metricsClient();
    await expect(
      observeUsageView(api, 'one', signal, true, 'latest', () => Promise.reject(error)),
    ).rejects.toBe(error);
    expect(viewUnavailable(api, 'one', true, 'latest')).toBe(false);
  },
);
