import { expect, it } from 'vitest';
import { transport } from './resource-fixtures.js';
import { ProtocolError, ValidationError } from '../src/index.js';

const settings = {
  model: 'gpt-6',
  reasoning_effort: 'high',
  permission_mode: 'autonomous',
} as const;
it('preserves absolute settings and command identity across retries and reads pending recovery', async () => {
  const test = transport();
  test.answer({ ...settings, pending: false });
  for (let attempt = 0; attempt < 2; attempt++)
    expect(
      await test.client.agents.updateSettings('agent-one', settings, {
        idempotencyKey: 'settings-one',
      }),
    ).toEqual({ ...settings, pending: false });
  for (const call of test.calls) {
    expect(call.url).toBe('https://cma.example.test/api/v1/agents/agent-one/settings');
    expect(call.init.method).toBe('PATCH');
    expect(JSON.parse(String(call.init.body))).toEqual(settings);
    expect(new Headers(call.init.headers).get('Idempotency-Key')).toBe('settings-one');
  }
  test.answer({ ...settings, pending: true });
  expect((await test.client.agents.settings('agent-one')).pending).toBe(true);
  test.answer({ ...settings, model: '', pending: false });
  expect((await test.client.agents.settings('legacy')).model).toBe('');
});
it('refuses invalid model requests before transport and incomplete runtime receipts', async () => {
  const test = transport();
  for (const model of ['', ' padded ', '\n', 'a'.repeat(257), 'é'.repeat(129)])
    expect(() =>
      test.client.agents.updateSettings(
        'one',
        { ...settings, model },
        { idempotencyKey: 'settings' },
      ),
    ).toThrow(ValidationError);
  expect(test.calls).toEqual([]);
  test.answer(settings);
  await expect(test.client.agents.settings('one')).rejects.toBeInstanceOf(ProtocolError);
});
