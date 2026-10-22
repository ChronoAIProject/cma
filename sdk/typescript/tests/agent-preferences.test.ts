import { expect, it } from 'vitest';
import { transport } from './resource-fixtures.js';
import { ProtocolError, ValidationError } from '../src/index.js';

it('preserves revision, owner prompt and exact retry identity', async () => {
  const test = transport();
  const receipt = {
    title: 'Research',
    system_prompt: 'Keep citations.',
    prompt_pending: false,
    prompt_supported: true,
    revision: 1,
  };
  test.answer(receipt);
  const body = { expected_revision: 0, system_prompt: 'Keep citations.' };
  for (let attempt = 0; attempt < 2; attempt++)
    expect(
      await test.client.agents.updatePreferences('one', body, { idempotencyKey: 'prompt-one' }),
    ).toEqual(receipt);
  for (const call of test.calls) {
    expect(call.url).toBe('https://cma.example.test/api/v1/agents/one/preferences');
    expect(call.init.method).toBe('PATCH');
    expect(JSON.parse(String(call.init.body))).toEqual(body);
    expect(new Headers(call.init.headers).get('Idempotency-Key')).toBe('prompt-one');
  }
  expect(await test.client.agents.preferences('one')).toEqual(receipt);
});
it('rejects ambiguous or oversized edits and incomplete runtime confirmation receipts', async () => {
  const test = transport();
  for (const body of [
    { expected_revision: 0 },
    { expected_revision: 0, title: 'Title', system_prompt: '' },
    { expected_revision: 0, title: 'a'.repeat(65) },
    { expected_revision: 0, system_prompt: 'é'.repeat(16385) },
  ])
    expect(() =>
      test.client.agents.updatePreferences('one', body, { idempotencyKey: 'one' }),
    ).toThrow(ValidationError);
  expect(test.calls).toHaveLength(0);
  test.answer({ revision: 1, system_prompt: '' });
  await expect(test.client.agents.preferences('one')).rejects.toBeInstanceOf(ProtocolError);
});
