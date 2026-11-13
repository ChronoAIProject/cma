import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { harness } from './controller.fixture';
import { agentTransportJournal } from './journal';

beforeEach(() => sessionStorage.clear());
afterEach(() => vi.restoreAllMocks());

it.each([429, 503])(
  'does not persist a legacy selection when HTTP %s supplies Retry-After',
  async (status) => {
    const h = harness();
    h.eligibility.mockResolvedValue(
      new Response(
        JSON.stringify({
          status,
          code: 'temporarily_busy',
          detail: 'Try again later.',
        }),
        { status, headers: { 'content-type': 'application/problem+json', 'retry-after': '45' } },
      ),
    );
    await expect(h.client.agentTransport!.open('agt_1')).rejects.toMatchObject({
      status,
      retryAfterMs: 45000,
    });
    expect(agentTransportJournal(sessionStorage, 'alice', 'agt_1').selected()).toBeUndefined();
    expect(h.eligibility).toHaveBeenCalledOnce();
    expect(h.post).not.toHaveBeenCalled();
  },
);

it('preserves unsupported-server fallback when eligibility has no retry instruction', async () => {
  const h = harness();
  h.eligibility.mockResolvedValue(
    new Response(
      JSON.stringify({
        status: 404,
        code: 'not_found',
        detail: 'Unavailable on this server.',
      }),
      { status: 404, headers: { 'content-type': 'application/problem+json' } },
    ),
  );
  await expect(h.client.agentTransport!.open('agt_1')).resolves.toBeUndefined();
  expect(agentTransportJournal(sessionStorage, 'alice', 'agt_1').selected()).toBe('agui');
});
