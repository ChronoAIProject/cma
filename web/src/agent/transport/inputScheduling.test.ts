import { beforeEach, expect, it, vi } from 'vitest';
import { harness, json, completedInputReceipt } from './controller.fixture';
import { isIdentityChanged } from './identity';

beforeEach(() => sessionStorage.clear());

async function opened() {
  const h = harness();
  const session = (await h.client.agentTransport!.open('agt_1'))!;
  h.currentUser.mockClear();
  return { ...h, session };
}

async function holdBackground(h: Awaited<ReturnType<typeof opened>>) {
  let release!: (user: string) => void;
  h.currentUser.mockReturnValueOnce(
    new Promise((resolve) => {
      release = resolve;
    }),
  );
  const background = h.session.authorize();
  await Promise.resolve();
  return async () => {
    release('alice');
    await background;
  };
}

it('reconciles an admission conflict with the same body and key while background authority is held', async () => {
  const h = await opened();
  const release = await holdBackground(h);
  h.post.mockResolvedValueOnce(
    json(
      {
        error: {
          type: 'invalid_request_error',
          code: 'agent_command_admission_conflict',
          message: 'Contention',
          param: null,
        },
      },
      409,
    ),
  );
  const onAccepted = vi.fn();
  try {
    await expect(h.session.send('original input', onAccepted)).rejects.toThrow();
    h.inputReceipt.mockImplementationOnce(async () => completedInputReceipt());
    await h.session.recover(onAccepted);
    expect(h.currentUser).toHaveBeenCalledTimes(4);
    expect(onAccepted).toHaveBeenCalledOnce();
    expect(h.post).toHaveBeenCalledOnce();
    expect(h.inputReceipt).toHaveBeenCalledOnce();
    const first = h.post.mock.lastCall![0]!;
    const second = h.inputReceipt.mock.lastCall![0]!;
    expect(second!.body).toBe(first!.body);
    expect(new Headers(second!.headers).get('idempotency-key')).toBe(
      new Headers(first!.headers).get('idempotency-key'),
    );
  } finally {
    await release();
  }
});

it('recovers lost admission with exact input, answer context and key on the input lane', async () => {
  const h = await opened();
  h.post.mockRejectedValueOnce(new TypeError('Lost receipt'));
  const context = { answer_sequence: 3, turn_sequence: 1 };
  await expect(
    h.session.send('original input', vi.fn(), undefined, undefined, context),
  ).rejects.toThrow();
  const release = await holdBackground(h);
  const onAccepted = vi.fn();
  try {
    h.inputReceipt.mockImplementationOnce(async () => completedInputReceipt());
    await h.session.forSnapshotRefresh().recover(onAccepted);
    expect(onAccepted).toHaveBeenCalledOnce();
    const first = h.post.mock.lastCall![0]!;
    const second = h.inputReceipt.mock.lastCall![0]!;
    // Journal decoding canonicalizes field order; the complete request values
    // and idempotency key must survive recovery unchanged.
    expect(JSON.parse(String(second!.body))).toStrictEqual(JSON.parse(String(first!.body)));
    expect(JSON.parse(String(second!.body)).answer_context).toEqual(context);
    expect(new Headers(second!.headers).get('idempotency-key')).toBe(
      new Headers(first!.headers).get('idempotency-key'),
    );
    await h.session.recover(onAccepted);
    expect(h.post).toHaveBeenCalledOnce();
    expect(h.inputReceipt).toHaveBeenCalledOnce();
  } finally {
    await release();
  }
});

it.each([1, 2])(
  'refuses unavailable identity at Send boundary %s without accepting',
  async (boundary) => {
    const h = await opened();
    let checks = 0;
    h.currentUser.mockImplementation(async () => {
      if (++checks === boundary) throw new Error('identity unavailable');
      return 'alice';
    });
    const onAccepted = vi.fn();
    await expect(h.session.send('input', onAccepted)).rejects.toThrow();
    expect(h.post).toHaveBeenCalledTimes(boundary === 1 ? 0 : 1);
    expect(onAccepted).not.toHaveBeenCalled();
    expect(h.session.journal.read()?.state ?? null).toBe(boundary === 1 ? null : 'pending');
  },
);

it.each([1, 2])('refuses changed accounts at Send boundary %s', async (boundary) => {
  const h = await opened();
  let checks = 0;
  h.currentUser.mockImplementation(async () => (++checks === boundary ? 'bob' : 'alice'));
  const onAccepted = vi.fn();
  const error = await h.session.send('input', onAccepted).catch((error: unknown) => error);
  expect(isIdentityChanged(error)).toBe(true);
  expect(h.post).toHaveBeenCalledTimes(boundary === 1 ? 0 : 1);
  expect(onAccepted).not.toHaveBeenCalled();
});

it('aborts one Send subscriber without cancelling a peer sharing its identity read', async () => {
  const h = await opened();
  const second = (await h.client.agentTransport!.open('agt_1'))!;
  let release!: (user: string) => void;
  h.currentUser.mockReturnValueOnce(
    new Promise((resolve) => {
      release = resolve;
    }),
  );
  const controller = new AbortController();
  const rejected = expect(
    h.session.send('aborted input', vi.fn(), controller.signal),
  ).rejects.toMatchObject({ name: 'AbortError' });
  const onAccepted = vi.fn();
  const retained = second.send('retained input', onAccepted);
  await Promise.resolve();
  const signal = h.currentUser.mock.lastCall![0]!;
  controller.abort();
  await rejected;
  expect(signal.aborted).toBe(false);
  release('alice');
  await retained;
  expect(h.post).toHaveBeenCalledOnce();
  expect(h.session.journal.read()).toMatchObject({ input: 'retained input', state: 'accepted' });
});

it('retains uncertain admission when navigation aborts after POST and recovers the same key', async () => {
  const h = await opened();
  let reached!: () => void;
  const ready = new Promise<void>((resolve) => {
    reached = resolve;
  });
  h.post.mockImplementationOnce(
    (init) =>
      new Promise((_resolve, reject) => {
        reached();
        init!.signal!.addEventListener(
          'abort',
          () => reject(new DOMException('Aborted', 'AbortError')),
          { once: true },
        );
      }),
  );
  const controller = new AbortController();
  const onAccepted = vi.fn();
  const rejected = expect(h.session.send('input', onAccepted, controller.signal)).rejects.toThrow();
  await ready;
  controller.abort();
  await rejected;
  expect(onAccepted).not.toHaveBeenCalled();
  expect(h.session.journal.read()?.state).toBe('pending');
  h.inputReceipt.mockImplementationOnce(async () => completedInputReceipt());
  await h.session.recover(onAccepted);
  const first = h.post.mock.lastCall![0]!;
  const second = h.inputReceipt.mock.lastCall![0]!;
  expect(second!.body).toBe(first!.body);
  expect(new Headers(second!.headers).get('idempotency-key')).toBe(
    new Headers(first!.headers).get('idempotency-key'),
  );
  expect(onAccepted).toHaveBeenCalledOnce();
});
