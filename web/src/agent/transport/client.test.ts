import { beforeEach, expect, it } from 'vitest';
import { harness, json } from './controller.fixture';

beforeEach(() => sessionStorage.clear());

it('uses authenticated eligibility for new tabs and preserves each selected transport across flag changes', async () => {
  const h = harness();
  const eligibility: unknown = await (await h.eligibility()).json();
  h.eligibility.mockClear();
  h.eligibility.mockResolvedValueOnce(
    json({
      ...(eligibility as Record<string, unknown>),
      enabled: false,
    }),
  );
  expect(await h.client.agentTransport!.open('legacy')).toBeUndefined();
  expect(await h.client.agentTransport!.open('v2')).toBeDefined();
  expect(h.eligibility).toHaveBeenCalledTimes(2);
  h.eligibility.mockRejectedValue(new TypeError('flag service unavailable'));
  expect(await h.client.agentTransport!.open('v2')).toBeDefined();
  expect(await h.client.agentTransport!.open('legacy')).toBeUndefined();
  expect(h.eligibility).toHaveBeenCalledTimes(2);
  expect(h.post).not.toHaveBeenCalled();
});

it('keeps a new tab on the legacy transport when the optional eligibility endpoint is absent', async () => {
  const h = harness();
  h.eligibility.mockResolvedValueOnce(
    json(
      {
        error: {
          type: 'invalid_request_error',
          code: 'not_found',
          message: 'Unknown endpoint',
          param: null,
        },
      },
      404,
    ),
  );
  expect(await h.client.agentTransport!.open('legacy')).toBeUndefined();
  expect(h.snapshot).not.toHaveBeenCalled();
  expect(h.post).not.toHaveBeenCalled();
  expect(await h.client.agentTransport!.open('legacy')).toBeUndefined();
  expect(h.eligibility).toHaveBeenCalledOnce();
});
