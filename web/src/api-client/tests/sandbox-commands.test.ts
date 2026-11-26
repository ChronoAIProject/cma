// @vitest-environment node
import { expect, it, vi } from 'vitest';
import { createClient, ProtocolError, ValidationError } from '../index.js';
import { command, now, transport } from './resource-fixtures.js';

const request = { command: 'pause', expected_revision: 7 } as const;
const queued = {
  command_id: 'scmd_one',
  sandbox_id: 'sandbox-1',
  ...request,
  admitted_at: now,
  queue_deadline: now,
  revision: 1,
  state: { status: 'queued' },
};

function barrier() {
  let release!: () => void;
  const wait = new Promise<void>((resolve) => {
    release = resolve;
  });
  return { wait, release };
}

it('sends the submitted command when the caller edits it before encoding', async () => {
  const body = { command: 'pause' as 'pause' | 'revive', expected_revision: 7 };
  const resolvingRequestId = barrier();
  const requestId = barrier();
  const requestIdProvider = vi.fn(async () => {
    resolvingRequestId.release();
    await requestId.wait;
    return 'held-request';
  });
  const fetch = vi.fn<typeof globalThis.fetch>(async (_url, options) => {
    const received = JSON.parse(String(options?.body)) as typeof body;
    return Response.json({ ...queued, ...received }, { status: 202 });
  });
  const client = createClient({ baseUrl: 'https://cma.example.test', fetch, requestIdProvider });
  const submitted = { idempotencyKey: command.idempotencyKey };
  const result = client.sandboxes.submitCommand('sandbox-1', body, submitted);
  await resolvingRequestId.wait;
  expect(requestIdProvider).toHaveBeenCalledOnce();
  expect(fetch).not.toHaveBeenCalled();

  body.command = 'revive';
  body.expected_revision = 8;
  submitted.idempotencyKey = 'changed-key';
  requestId.release();
  const receipt = await result;

  expect(fetch).toHaveBeenCalledOnce();
  const options = fetch.mock.calls[0]?.[1];
  expect(options?.method).toBe('POST');
  expect(JSON.parse(String(options?.body))).toEqual(request);
  expect(new Headers(options?.headers).get('Idempotency-Key')).toBe(command.idempotencyKey);
  expect(receipt).toEqual(queued);
});

it.each([
  { name: 'the submitted Pause', receipt: queued, matches: true },
  {
    name: 'the edited Revive',
    receipt: { ...queued, command: 'revive', expected_revision: 8 },
    matches: false,
  },
])(
  'checks the receipt for $name against the intent sent before the caller edit',
  async ({ receipt, matches }) => {
    const body = { command: 'pause' as 'pause' | 'revive', expected_revision: 7 };
    const posted = barrier();
    const response = barrier();
    const fetch = vi.fn<typeof globalThis.fetch>(async () => {
      posted.release();
      await response.wait;
      return Response.json(receipt, { status: 202 });
    });
    const client = createClient({ baseUrl: 'https://cma.example.test', fetch });
    const result = client.sandboxes.submitCommand('sandbox-1', body, command);
    await posted.wait;

    body.command = 'revive';
    body.expected_revision = 8;
    response.release();
    if (matches) await expect(result).resolves.toEqual(queued);
    else await expect(result).rejects.toBeInstanceOf(ProtocolError);

    expect(fetch).toHaveBeenCalledOnce();
    const options = fetch.mock.calls[0]?.[1];
    expect(options?.method).toBe('POST');
    expect(JSON.parse(String(options?.body))).toEqual(request);
    expect(new Headers(options?.headers).get('Idempotency-Key')).toBe(command.idempotencyKey);
  },
);

it('submits and recovers the original receipt and terminal states with the same key and precondition', async () => {
  const test = transport();
  for (const state of [
    { status: 'queued' },
    { status: 'applied', at: now, sandbox_revision: 8, disposition: 'scheduled' },
    { status: 'applied', at: now, sandbox_revision: 7, disposition: 'already_scheduled' },
    { status: 'applied', at: now, sandbox_revision: 7, disposition: 'already_satisfied' },
    { status: 'rejected', at: now, reason: 'authority_revoked' },
  ]) {
    const receipt = { ...queued, revision: state.status === 'queued' ? 1 : 2, state };
    test.answer(receipt, 202);
    expect(await test.client.sandboxes.submitCommand('sandbox-1', request, command)).toEqual(
      receipt,
    );
    const call = test.calls.at(-1)!;
    expect(call.url).toBe('https://cma.example.test/api/v1/sandboxes/sandbox-1/commands');
    expect(JSON.parse(String(call.init.body))).toEqual(request);
    expect(new Headers(call.init.headers).get('Idempotency-Key')).toBe(command.idempotencyKey);
    test.answer(receipt);
    expect(await test.client.sandboxes.readCommand('scmd_one')).toEqual(receipt);
    expect(test.calls.at(-1)?.url).toBe(
      'https://cma.example.test/api/v1/sandbox-commands/scmd_one',
    );
  }
});

it('refuses malformed receipts and request mismatches before reporting an outcome', async () => {
  const test = transport();
  for (const patch of [
    { command_id: '' },
    { sandbox_id: '' },
    { sandbox_id: 'another' },
    { command: 'retire' },
    { command: 'future' },
    { expected_revision: 8 },
    { expected_revision: 0 },
    { revision: 2 },
    { state: { status: 'future' } },
    {
      revision: 2,
      state: { status: 'applied', at: now, sandbox_revision: 7, disposition: 'scheduled' },
    },
    {
      revision: 2,
      state: { status: 'applied', at: now, sandbox_revision: 8, disposition: 'already_satisfied' },
    },
    { revision: 2, state: { status: 'rejected', at: now, reason: 'unknown' } },
    { revision: 2, state: { status: 'rejected', reason: 'queue_expired' } },
  ]) {
    test.answer({ ...queued, ...patch }, 202);
    await expect(
      test.client.sandboxes.submitCommand('sandbox-1', request, command),
    ).rejects.toBeInstanceOf(ProtocolError);
  }
  test.answer({ ...queued, command_id: 'scmd_another' });
  await expect(test.client.sandboxes.readCommand('scmd_one')).rejects.toBeInstanceOf(ProtocolError);
  test.answer(queued);
  await expect(
    test.client.sandboxes.submitCommand('sandbox-1', request, command),
  ).rejects.toBeInstanceOf(ProtocolError);
});

it('rejects invalid revision preconditions locally', async () => {
  const test = transport();
  for (const expected_revision of [0, -1, 1.5, Number.MAX_SAFE_INTEGER + 1])
    expect(() =>
      test.client.sandboxes.submitCommand('sandbox-1', { ...request, expected_revision }, command),
    ).toThrow(ValidationError);
  expect(test.calls).toHaveLength(0);
});

it('refuses a JSON encoder that changes the validated command', () => {
  const test = transport();
  const body = {
    ...request,
    toJSON: () => ({ command: 'revive', expected_revision: 8 }),
  };
  expect(() => test.client.sandboxes.submitCommand('sandbox-1', body, command)).toThrow(
    ValidationError,
  );
  expect(test.calls).toHaveLength(0);
});

it('retains an additive bounded queue result without confusing it with completion', async () => {
  const test = transport();
  const result = {
    state: 'recoverable_pending',
    code: 'lifecycle_wait_exceeded',
    operation_id: 'scmd_one',
    started_at: now,
    result_due_at: now,
    continuing: true,
    actions: ['observe'],
    retry_after_seconds: 5,
  };
  test.answer({ ...queued, lifecycle: { result } });
  const receipt = await test.client.sandboxes.readCommand('scmd_one');
  expect(receipt.lifecycle?.result).toEqual(result);
  expect(receipt.state.status).toBe('queued');
});
