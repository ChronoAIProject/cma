import { expect, it } from 'vitest';
import { ProtocolError, ValidationError } from '../src/index.js';
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
