import { beforeEach, expect, it } from 'vitest';
import { harness, json } from './controller.fixture';
import { controlReceipt } from './controls.fixture';
import { recoverV2Control, submitV2Control } from './controls';
import { IdentityChangedError } from './identity';

beforeEach(() => sessionStorage.clear());
const target = { responseId: 'resp_1', commandId: 'cmd_1', turnSequence: 1 };
const command = { type: 'cancel' } as const;

async function opened() {
  const h = harness();
  const session = (await h.client.agentTransport!.open('agt_1'))!;
  h.currentUser.mockClear();
  return { ...h, session };
}

it('records one Cancel while an unrelated identity read is still outstanding, retaining every fresh barrier', async () => {
  const h = await opened();
  let release!: (user: string) => void;
  h.currentUser.mockReturnValueOnce(
    new Promise((resolve) => {
      release = resolve;
    }),
  );
  const background = h.session.authorize();
  await Promise.resolve();
  await submitV2Control(h.session, target, command, 'original', new AbortController().signal);
  expect(h.currentUser).toHaveBeenCalledTimes(4);
  expect(h.observe).toHaveBeenCalledOnce();
  expect(h.control).toHaveBeenCalledOnce();
  expect(h.session.journal.readControl()).toMatchObject({
    key: 'original',
    commandId: 'cmd_control',
  });
  release('alice');
  await background;
});

it('recovers an uncertain Cancel with its exact original intent while a background read remains outstanding', async () => {
  const h = await opened();
  h.control.mockRejectedValueOnce(new TypeError('lost receipt'));
  await expect(
    submitV2Control(h.session, target, command, 'original', new AbortController().signal),
  ).rejects.toThrow();
  let release!: (user: string) => void;
  h.currentUser.mockReturnValueOnce(
    new Promise((resolve) => {
      release = resolve;
    }),
  );
  const background = h.session.authorize();
  await Promise.resolve();
  await recoverV2Control(h.session.forSnapshotRefresh(), new AbortController().signal);
  expect(h.observe).toHaveBeenCalledOnce();
  expect(h.control).toHaveBeenCalledTimes(2);
  const [first, recovered] = h.control.mock.calls;
  expect(recovered![0]).toBe(first![0]);
  expect(recovered![1]!.body).toBe(first![1]!.body);
  expect(new Headers(recovered![1]!.headers).get('idempotency-key')).toBe('original');
  release('alice');
  await background;
});

it.each([1, 2, 3])(
  'preserves account-change refusal at Cancel identity boundary %s',
  async (boundary) => {
    const h = await opened();
    let checks = 0;
    h.currentUser.mockImplementation(async () => (++checks === boundary ? 'bob' : 'alice'));
    await expect(
      submitV2Control(h.session, target, command, 'original', new AbortController().signal),
    ).rejects.toBeInstanceOf(IdentityChangedError);
    expect(h.control).toHaveBeenCalledTimes(boundary === 3 ? 1 : 0);
    expect(() => h.session.journal.readControl()).toThrow(IdentityChangedError);
  },
);

it.each([1, 2, 3])(
  'preserves identity-unavailable refusal at Cancel boundary %s without claiming acceptance',
  async (boundary) => {
    const h = await opened();
    let checks = 0;
    h.currentUser.mockImplementation(async () => {
      if (++checks === boundary) throw new Error('identity unavailable');
      return 'alice';
    });
    await expect(
      submitV2Control(h.session, target, command, 'original', new AbortController().signal),
    ).rejects.toThrow('identity unavailable');
    expect(h.control).toHaveBeenCalledTimes(boundary === 3 ? 1 : 0);
    expect(h.session.journal.readControl()?.commandId ?? null).toBeNull();
  },
);

it('aborts before dispatch while preserving the retained Cancel for explicit recovery', async () => {
  const h = await opened();
  let release!: (user: string) => void;
  let reached!: () => void;
  const ready = new Promise<void>((resolve) => {
    reached = resolve;
  });
  h.currentUser.mockResolvedValueOnce('alice').mockImplementationOnce(() => {
    reached();
    return new Promise((resolve) => {
      release = resolve;
    });
  });
  const controller = new AbortController();
  const rejected = expect(
    submitV2Control(h.session, target, command, 'original', controller.signal),
  ).rejects.toMatchObject({ name: 'AbortError' });
  await ready;
  controller.abort();
  await rejected;
  release('alice');
  expect(h.control).not.toHaveBeenCalled();
  expect(h.session.journal.readControl()).toMatchObject({ key: 'original', commandId: null });
});

it('retains an aborted dispatched Cancel and recovers the same key without a second observation', async () => {
  const h = await opened();
  let release!: () => void;
  let reached!: () => void;
  const ready = new Promise<void>((resolve) => {
    reached = resolve;
  });
  h.control.mockImplementationOnce((responseId, init) => {
    reached();
    return new Promise((resolve) => {
      release = () =>
        resolve(json(controlReceipt(responseId, JSON.parse(String(init?.body))), 202));
    });
  });
  const controller = new AbortController();
  const rejected = expect(
    submitV2Control(h.session, target, command, 'original', controller.signal),
  ).rejects.toThrow();
  await ready;
  controller.abort();
  await rejected;
  release();
  await recoverV2Control(h.session, new AbortController().signal);
  expect(h.observe).toHaveBeenCalledOnce();
  expect(h.control).toHaveBeenCalledTimes(2);
  expect(h.control.mock.calls[1]![1]!.body).toBe(h.control.mock.calls[0]![1]!.body);
  expect(new Headers(h.control.mock.calls[1]![1]!.headers).get('idempotency-key')).toBe('original');
});
