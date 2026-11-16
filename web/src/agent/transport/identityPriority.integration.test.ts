import { beforeEach, expect, it, vi } from 'vitest';
import { createClient } from '@chronoai/cma-sdk';
import { createAgentTransport } from './client';
import { agentTransportJournal, clearAgentTransport } from './journal';
import type { CurrentUser } from './identityGate';

beforeEach(() => sessionStorage.clear());
async function setup(enabled: boolean) {
  const currentUser = vi.fn<CurrentUser>(async () => 'owner');
  const post = vi.fn<typeof fetch>(
    async () =>
      new Response(new ReadableStream(), {
        headers: {
          'content-type': 'text/event-stream',
          'x-cma-response-id': 'fixture-response',
          'x-cma-command-id': 'fixture-command',
        },
      }),
  );
  const sdk = createClient({ fetch: post });
  agentTransportJournal(sessionStorage, 'owner', 'agt_1').select(true);
  const transport = createAgentTransport(sdk, {
    storage: sessionStorage,
    currentUser,
    foregroundIdentityPriority: enabled,
  });
  const session = (await transport.open('agt_1'))!;
  currentUser.mockClear();
  return { currentUser, post, session, transport };
}

for (const enabled of [false, true]) {
  it(`keeps two fresh checks and a durable receipt with priority ${enabled}`, async () => {
    const h = await setup(enabled);
    const accepted = vi.fn();
    await h.session.send('fixture', accepted);
    expect(h.currentUser).toHaveBeenCalledTimes(2);
    expect(h.post).toHaveBeenCalledOnce();
    expect(accepted).toHaveBeenCalledOnce();
    expect(h.currentUser.mock.invocationCallOrder[0]).toBeLessThan(
      h.post.mock.invocationCallOrder[0]!,
    );
    expect(h.currentUser.mock.invocationCallOrder[1]).toBeGreaterThan(
      h.post.mock.invocationCallOrder[0]!,
    );
  });
  it.each(['before', 'after', 'epoch', 'unauthenticated'] as const)(
    `refuses %s authority changes with priority ${enabled}`,
    async (failure) => {
      const h = await setup(enabled);
      if (failure === 'before') h.currentUser.mockResolvedValue('other-owner');
      if (failure === 'after')
        h.currentUser.mockResolvedValueOnce('owner').mockResolvedValueOnce('other-owner');
      if (failure === 'epoch')
        h.currentUser.mockImplementationOnce(async () => {
          clearAgentTransport(sessionStorage);
          return 'owner';
        });
      if (failure === 'unauthenticated') h.currentUser.mockResolvedValue(undefined);
      const accepted = vi.fn();
      await expect(h.session.send('fixture', accepted)).rejects.toThrow();
      expect(accepted).not.toHaveBeenCalled();
      expect(h.post).toHaveBeenCalledTimes(failure === 'after' ? 1 : 0);
    },
  );
  it(`does not accept a response without a durable receipt with priority ${enabled}`, async () => {
    const h = await setup(enabled);
    h.post.mockResolvedValueOnce(
      new Response('', { headers: { 'content-type': 'text/event-stream' } }),
    );
    const accepted = vi.fn();
    await expect(h.session.send('fixture', accepted)).rejects.toThrow();
    expect(accepted).not.toHaveBeenCalled();
    expect(h.post).toHaveBeenCalledOnce();
  });
}

it('keeps explicit initial selection ahead of queued background verification', async () => {
  const h = await setup(true);
  // Complete one background check so the earlier opening is not counted in
  // this scenario's two-foreground fairness allowance.
  await h.session.authorize();
  let release!: (value: string) => void;
  h.currentUser.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        release = resolve;
      }),
  );
  const foreground = h.session.authorizeCancel();
  for (let i = 0; i < 15; i++) await Promise.resolve();
  const order: string[] = [];
  const background = h.session.authorize().then(() => order.push('background'));
  agentTransportJournal(sessionStorage, 'owner', 'agt_2').select(true);
  const opening = h.transport.open('agt_2').then(() => order.push('open'));
  for (let i = 0; i < 15; i++) await Promise.resolve();
  release('owner');
  await Promise.all([foreground, background, opening]);
  expect(order).toEqual(['open', 'background']);
  expect(h.post).not.toHaveBeenCalled();
});

const flush = async () => {
  for (let i = 0; i < 25; i++) await Promise.resolve();
};

it('keeps background, Cancel and Send gates concurrent when priority is disabled', async () => {
  const h = await setup(false);
  const releases: Array<(user: string) => void> = [];
  h.currentUser.mockImplementation(() => new Promise((resolve) => releases.push(resolve)));
  const background = h.session.authorize();
  const cancel = h.session.authorizeCancel();
  const accepted = vi.fn();
  const send = h.session.send('fixture', accepted);
  await flush();
  expect(h.currentUser).toHaveBeenCalledTimes(3);
  expect(h.post).not.toHaveBeenCalled();
  h.currentUser.mockResolvedValue('owner');
  for (const release of releases) release('owner');
  await Promise.all([background, cancel, send]);
  expect(h.currentUser).toHaveBeenCalledTimes(4);
  expect(h.post).toHaveBeenCalledOnce();
  expect(accepted).toHaveBeenCalledOnce();
});

it('selects a fresh Send check ahead of queued background work after an active probe', async () => {
  const h = await setup(true);
  await h.session.authorize();
  let release!: (user: string) => void;
  h.currentUser.mockImplementationOnce(() => new Promise((resolve) => (release = resolve)));
  const cancel = h.session.authorizeCancel();
  await flush();
  const order: string[] = [];
  const background = h.session.authorize().then(() => order.push('background'));
  const reply = await h.post.getMockImplementation()!('/unused');
  h.post.mockImplementationOnce(async () => {
    order.push('post');
    return reply;
  });
  const send = h.session.send('fixture', () => order.push('accepted'));
  await flush();
  expect(h.currentUser).toHaveBeenCalledTimes(2);
  expect(h.post).not.toHaveBeenCalled();
  release('owner');
  await Promise.all([cancel, background, send]);
  expect(order[0]).toBe('post');
  expect(order).toContain('background');
  expect(order.at(-1)).toBe('accepted');
  expect(h.currentUser).toHaveBeenCalledTimes(5);
  expect(h.post).toHaveBeenCalledOnce();
});

it('cancels a queued Send before its verifier or POST starts', async () => {
  const h = await setup(true);
  let release!: (user: string) => void;
  h.currentUser.mockImplementationOnce(() => new Promise((resolve) => (release = resolve)));
  const background = h.session.authorize();
  await flush();
  const abort = new AbortController();
  const accepted = vi.fn();
  const send = h.session.send('fixture', accepted, abort.signal).catch((error: unknown) => error);
  await flush();
  const reason = new Error('cancelled queued Send');
  abort.abort(reason);
  expect(await send).toBe(reason);
  release('owner');
  await background;
  await flush();
  expect(h.currentUser).toHaveBeenCalledOnce();
  expect(h.post).not.toHaveBeenCalled();
  expect(accepted).not.toHaveBeenCalled();
});

it('refuses a same-owner epoch change after the durable receipt', async () => {
  const h = await setup(true);
  h.currentUser.mockResolvedValueOnce('owner').mockImplementationOnce(async () => {
    clearAgentTransport(sessionStorage);
    return 'owner';
  });
  const accepted = vi.fn();
  await expect(h.session.send('fixture', accepted)).rejects.toThrow();
  expect(h.post).toHaveBeenCalledOnce();
  expect(h.currentUser).toHaveBeenCalledTimes(2);
  expect(accepted).not.toHaveBeenCalled();
});
