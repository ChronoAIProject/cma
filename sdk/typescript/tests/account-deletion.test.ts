import { describe, expect, it } from 'vitest';
import { ApiError, ProtocolError, ValidationError } from '../src/index.js';
import { transport } from './resource-fixtures.js';

const receipt = {
  id: 'acd_0123456789abcdef0123456789abcdef',
  status: 'running',
  step: 'inventory',
  requested_at: '2026-11-14T00:00:00Z',
  deadline_at: '2026-11-21T00:00:00Z',
  completed_at: null,
  steps: [],
};

describe('self-service account deletion', () => {
  it('sends an explicit irreversible acknowledgement with the caller replay key', async () => {
    const test = transport();
    for (let attempt = 0; attempt < 2; attempt += 1) {
      test.answer(receipt, 202);
      expect(
        await test.client.account.deletion.request({ idempotencyKey: 'erase-own-data' }),
      ).toEqual(receipt);
      const call = test.calls.at(-1)!;
      expect(call.url).toContain('/api/v1/account/deletion');
      expect(call.init.method).toBe('POST');
      expect(JSON.parse(String(call.init.body))).toEqual({ acknowledge: 'irreversible' });
      expect(new Headers(call.init.headers).get('Idempotency-Key')).toBe('erase-own-data');
    }
  });

  it('reads progress and preserves typed refusals without claiming completion', async () => {
    const test = transport();
    test.answer(receipt);
    expect(await test.client.account.deletion.status()).toEqual(receipt);
    expect(test.calls.at(-1)!.init.method).toBe('GET');
    test.answer({ code: 'account_deletion_not_found', detail: 'No request exists.' }, 404);
    await expect(test.client.account.deletion.status()).rejects.toMatchObject({
      status: 404,
      code: 'account_deletion_not_found',
    });
    test.answer({ code: 'account_deletion_unavailable', detail: 'The writer is disabled.' }, 503);
    await expect(
      test.client.account.deletion.request({ idempotencyKey: 'erase-own-data' }),
    ).rejects.toBeInstanceOf(ApiError);
  });

  it('recognizes the server advertised deletion entitlement', async () => {
    const test = transport();
    test.answer({
      authenticated: true,
      subject: 'owner',
      capabilities: ['delete_own_account'],
      visitor: false,
    });
    await expect(test.client.auth.readSession()).resolves.toMatchObject({
      authenticated: true,
      capabilities: ['delete_own_account'],
    });
  });

  it('refuses invalid keys before HTTP and rejects unknown states or malformed progress', async () => {
    const test = transport();
    await expect(
      test.client.account.deletion.request({ idempotencyKey: 'invalid key' }),
    ).rejects.toBeInstanceOf(ValidationError);
    expect(test.calls).toHaveLength(0);
    for (const value of [
      { ...receipt, status: 'deleted' },
      { ...receipt, step: 'done' },
      { ...receipt, completed_at: undefined },
      {
        ...receipt,
        steps: [{ step: 'inventory', completed_at: receipt.requested_at, counts: { records: -1 } }],
      },
    ]) {
      test.answer(value);
      await expect(test.client.account.deletion.status()).rejects.toBeInstanceOf(ProtocolError);
    }
    test.answer(receipt, 200);
    await expect(
      test.client.account.deletion.request({ idempotencyKey: 'erase-own-data' }),
    ).rejects.toBeInstanceOf(ProtocolError);
  });
});
