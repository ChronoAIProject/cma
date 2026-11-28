import { beforeEach, expect, it, vi } from 'vitest';
import { accepted, harness, json } from './controller.fixture';
import { commandStatus } from './controls.fixture';
import { clearAgentTransport } from './journal';

const feature = vi.hoisted(() => ({ enabled: true }));
vi.mock('./recoveryConfig', () => ({
  get inputRefusalRecoveryEnabled() {
    return feature.enabled;
  },
}));

beforeEach(() => {
  sessionStorage.clear();
  feature.enabled = true;
});

async function setup(enabled = false) {
  const h = harness(sessionStorage, { enabled, delay: () => 0 });
  const open = async () => (await h.client.agentTransport!.open('agt_1'))!;
  return { ...h, open, session: await open() };
}

function refusal(code = 'rate_limited') {
  const response = json({ status: 429, title: 'Rate limited', code }, 429);
  response.headers.set('retry-after', '20');
  return response;
}

const key = (init?: RequestInit) => new Headers(init?.headers).get('idempotency-key');

it.each([false, true])(
  'reconciles the original refusal with replica recovery %s',
  async (enabled) => {
    const h = await setup(enabled);
    h.post.mockResolvedValueOnce(refusal());
    h.inputReceipt.mockResolvedValueOnce(json(null));
    const onAccepted = vi.fn();
    expect(
      await h.session.send('draft', onAccepted, undefined, undefined, {
        turn_sequence: 1,
        answer_sequence: 2,
      }),
    ).toBe('refused');
    expect(h.session.journal.read()).toBeNull();
    expect(h.post).toHaveBeenCalledOnce();
    expect(h.inputReceipt).toHaveBeenCalledOnce();
    const [create] = h.post.mock.lastCall!;
    const [lookup] = h.inputReceipt.mock.lastCall!;
    expect(lookup?.body).toBe(create?.body);
    expect(key(lookup)).toBe(key(create));
    expect(onAccepted).not.toHaveBeenCalled();

    const reopened = await h.open();
    const retryAt = reopened.pending().inputRetryAt!;
    expect(retryAt).toBeGreaterThan(Date.now());
    expect(await reopened.send('edited draft', onAccepted)).toBe('refused');
    expect(h.post).toHaveBeenCalledOnce();
    const now = vi.spyOn(Date, 'now').mockReturnValue(retryAt);
    try {
      h.post.mockResolvedValueOnce(accepted());
      await reopened.send('edited draft', onAccepted);
      expect(h.post).toHaveBeenCalledTimes(2);
      expect(key(h.post.mock.lastCall?.[0])).not.toBe(key(create));
      expect(onAccepted).toHaveBeenCalledOnce();
    } finally {
      now.mockRestore();
    }
  },
);

it.each([
  [200, ''],
  [200, 'not json'],
  [204, undefined],
  [201, 'null'],
])('keeps custody on an inconclusive HTTP %s lookup', async (status, body) => {
  const h = await setup();
  h.post.mockResolvedValueOnce(refusal());
  h.inputReceipt.mockResolvedValueOnce(
    new Response(body, { status, headers: { 'content-type': 'application/json' } }),
  );
  await expect(h.session.send('draft', vi.fn())).rejects.toThrow();
  expect(h.session.journal.read()?.state).toBe('pending');
  const reopened = await h.open();
  h.inputReceipt.mockResolvedValueOnce(json(null));
  expect(await reopened.recover(vi.fn())).toBe('refused');
  expect(reopened.journal.read()).toBeNull();
  expect(h.post).toHaveBeenCalledOnce();
});

it('accepts the matching receipt and clears the refusal evidence', async () => {
  const h = await setup();
  h.post.mockResolvedValueOnce(refusal());
  h.inputReceipt.mockResolvedValueOnce(json(commandStatus('resp_1', 1, true)));
  const onAccepted = vi.fn();
  await h.session.send('draft', onAccepted);
  expect(onAccepted).toHaveBeenCalledOnce();
  expect(h.session.journal.read()?.state).toBe('accepted');
  expect(h.post).toHaveBeenCalledOnce();
  expect(h.session.journal.inputRefusal.matches(h.session.journal.read()!)).toBe(false);
});

it('keeps an ambiguous input pending after lookup-stage throttling and exact null', async () => {
  const h = await setup();
  h.post.mockRejectedValueOnce(new TypeError('connection lost'));
  await expect(h.session.send('draft', vi.fn())).rejects.toThrow();
  h.inputReceipt.mockResolvedValueOnce(refusal()).mockResolvedValueOnce(json(null));
  await expect(h.session.recover(vi.fn())).rejects.toThrow();
  expect(await h.session.recover(vi.fn())).toBe(false);
  expect(h.session.journal.read()?.state).toBe('pending');
  expect(h.post).toHaveBeenCalledOnce();
});

it('preserves legacy custody with input refusal recovery switched off', async () => {
  feature.enabled = false;
  const h = await setup();
  h.post.mockResolvedValueOnce(refusal());
  await expect(h.session.send('draft', vi.fn())).rejects.toThrow();
  expect(h.inputReceipt).not.toHaveBeenCalled();
  h.inputReceipt.mockResolvedValueOnce(json(null));
  expect(await h.session.recover(vi.fn())).toBe(false);
  expect(h.session.journal.read()?.state).toBe('pending');
});

it.each(['accepted', 'superseded', 'identity'] as const)(
  'does not release custody for a late null after the input became %s',
  async (change) => {
    const h = await setup();
    h.post.mockResolvedValueOnce(refusal());
    h.inputReceipt.mockRejectedValueOnce(new TypeError('lookup interrupted'));
    await expect(h.session.send('draft', vi.fn())).rejects.toThrow();
    const journal = h.session.journal;
    const { key: originalKey } = journal.read()!;
    h.inputReceipt.mockImplementationOnce(async () => {
      if (change === 'accepted')
        journal.accepted(originalKey, { responseId: 'resp_1', commandId: 'cmd_1' });
      else if (change === 'superseded') {
        journal.refused(originalKey);
        journal.retain('another input');
      } else clearAgentTransport(sessionStorage);
      return json(null);
    });
    if (change === 'identity') {
      await expect(h.session.recover(vi.fn())).rejects.toThrow();
    } else {
      expect(await h.session.recover(vi.fn())).toBe(false);
      expect(journal.read()?.state).toBe(change === 'accepted' ? 'accepted' : 'pending');
      expect(journal.read()?.input).toBe(change === 'accepted' ? 'draft' : 'another input');
    }
  },
);
