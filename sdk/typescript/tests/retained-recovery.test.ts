import { expect, it } from 'vitest';
import { ProtocolError, ValidationError } from '../src/index.js';
import { command, transport } from './resource-fixtures.js';

it('sends explicit same-image consent with the original revision and key on each retry', async () => {
  const f = transport();
  const body = { expected_revision: 7, confirm_same_image_recovery: true };
  const receipt = {
    operation_id: 'retained-reopen',
    revision: 8,
    accepted_at: '2026-11-11T04:00:00Z',
  };
  for (let i = 0; i < 2; i++) {
    f.answer(receipt, 202);
    expect(await f.client.sandboxes.recoverRetained('sbx_recovery', body, command)).toEqual(
      receipt,
    );
    const sent = f.calls.at(-1)!;
    expect(sent.url).toContain('/api/v1/sandboxes/sbx_recovery/runtime-recovery');
    expect(sent.init.method).toBe('POST');
    expect(JSON.parse(sent.init.body as string)).toEqual(body);
    expect(new Headers(sent.init.headers).get('idempotency-key')).toBe(command.idempotencyKey);
  }
  expect(() =>
    f.client.sandboxes.recoverRetained('sbx_recovery', { ...body, expected_revision: 0 }, command),
  ).toThrow(ValidationError);
  expect(() =>
    f.client.sandboxes.recoverRetained('sbx_recovery', body, { idempotencyKey: '' }),
  ).toThrow(ValidationError);
  f.answer(receipt, 200);
  await expect(
    f.client.sandboxes.recoverRetained('sbx_recovery', body, command),
  ).rejects.toBeInstanceOf(ProtocolError);
  f.answer({ ...receipt, revision: 0 }, 202);
  await expect(
    f.client.sandboxes.recoverRetained('sbx_recovery', body, command),
  ).rejects.toBeInstanceOf(ProtocolError);
});
