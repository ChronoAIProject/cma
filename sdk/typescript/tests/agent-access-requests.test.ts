import { expect, it } from 'vitest';
import { transport } from './resource-fixtures.js';
import { ProtocolError, ValidationError } from '../src/index.js';
const pending = {
  id: 'request-one',
  agent_id: 'agent-one',
  status: 'pending',
  services: ['api-github'],
  reason: 'Read repository',
};
it('projects durable requests and replays exact authenticated approval/denial commands', async () => {
  const test = transport();
  test.answer({ items: [pending] });
  expect(await test.client.agents.accessRequests('agent-one')).toEqual({ items: [pending] });
  const connecting = {
    ...pending,
    status: 'connecting',
    connection: { target: { type: 'agent_access_request', id: pending.id } },
  };
  test.answer(connecting);
  const body = { services: ['chrono-llm-public', 'ornn-api', 'api-github'] };
  for (let attempt = 0; attempt < 2; attempt++)
    expect(
      await test.client.agents.approveAccessRequest('agent-one', pending.id, body, {
        idempotencyKey: 'approve-one',
      }),
    ).toEqual(connecting);
  expect(test.calls[1]?.url).toBe(
    'https://cma.example.test/api/v1/agents/agent-one/access-requests/request-one/approve',
  );
  expect(test.calls[2]?.init.body).toBe(test.calls[1]?.init.body);
  expect(new Headers(test.calls[1]?.init.headers).get('Idempotency-Key')).toBe('approve-one');
  expect(JSON.parse(String(test.calls[1]?.init.body))).toEqual(body);
  test.answer({ ...pending, status: 'denied' });
  await test.client.agents.denyAccessRequest('agent-one', pending.id, {
    idempotencyKey: 'deny-one',
  });
  expect(test.calls[3]?.url.endsWith('/request-one/deny')).toBe(true);
  expect(test.calls[3]?.init.body).toBe('{}');
  expect(new Headers(test.calls[3]?.init.headers).get('Idempotency-Key')).toBe('deny-one');
});
it('rejects crossed Agent/request identities and incomplete connecting receipts', async () => {
  const test = transport();
  for (const wrong of [
    { ...pending, agent_id: 'another' },
    { ...pending, status: 'connecting' },
    { ...pending, connection: { target: { type: 'agent_access_request', id: 'another' } } },
  ]) {
    test.answer({ items: [wrong] });
    await expect(test.client.agents.accessRequests('agent-one')).rejects.toBeInstanceOf(
      ProtocolError,
    );
  }
  test.answer({ ...pending, id: 'another' });
  await expect(
    test.client.agents.denyAccessRequest('agent-one', pending.id, { idempotencyKey: 'deny-one' }),
  ).rejects.toBeInstanceOf(ProtocolError);
  test.answer({ items: [pending, pending] });
  await expect(test.client.agents.accessRequests('agent-one')).rejects.toBeInstanceOf(
    ProtocolError,
  );
});
it('validates service bounds and command identity before transport', async () => {
  const test = transport();
  for (const services of [
    [],
    ['a'.repeat(129)],
    ['github/read'],
    Array<string>(33).fill('service'),
  ])
    await expect(() =>
      test.client.agents.approveAccessRequest(
        'agent-one',
        pending.id,
        { services },
        { idempotencyKey: 'key' },
      ),
    ).toThrow(ValidationError);
  expect(() =>
    test.client.agents.denyAccessRequest('agent-one', pending.id, { idempotencyKey: '' }),
  ).toThrow(ValidationError);
  expect(test.calls).toEqual([]);
});
it('retains failed predecessor identity on a fresh pending consent attempt', async () => {
  const test = transport();
  const failed = { ...pending, id: `aar_${'a'.repeat(64)}`, status: 'failed' };
  const next = { ...pending, id: `aar_${'b'.repeat(64)}`, retry_of: failed.id };
  test.answer({ items: [next, failed] });
  expect(await test.client.agents.accessRequests('agent-one')).toEqual({ items: [next, failed] });
  for (const retry_of of ['foreign', next.id]) {
    test.answer({ items: [{ ...next, retry_of }] });
    await expect(test.client.agents.accessRequests('agent-one')).rejects.toBeInstanceOf(
      ProtocolError,
    );
  }
});
it('replaces exact shared scope with an observed-set guard and preserves replay identity', async () => {
  const test = transport();
  const body = {
    services: ['chrono-llm-public', 'ornn-api'],
    expected_services: ['chrono-llm-public', 'ornn-api', 'api-github'],
  };
  const result = { ...pending, services: body.services, status: 'applying' };
  test.answer(result, 202);
  for (let attempt = 0; attempt < 2; attempt++) {
    expect(
      await test.client.agents.updateServiceAccess('agent-one', body, {
        idempotencyKey: 'scope-one',
      }),
    ).toEqual(result);
  }
  expect(test.calls[0]?.url).toBe(
    'https://cma.example.test/api/v1/agents/agent-one/service-access',
  );
  expect(test.calls[0]?.init.method).toBe('PUT');
  expect(JSON.parse(String(test.calls[0]?.init.body))).toEqual(body);
  expect(test.calls[1]?.init.body).toEqual(test.calls[0]?.init.body);
  expect(new Headers(test.calls[1]?.init.headers).get('Idempotency-Key')).toBe('scope-one');
  test.answer({ ...result, agent_id: 'other' }, 202);
  await expect(
    test.client.agents.updateServiceAccess('agent-one', body, { idempotencyKey: 'scope-one' }),
  ).rejects.toBeInstanceOf(ProtocolError);
});
it('refuses invalid replacement selections before performing any network request', () => {
  const test = transport();
  for (const body of [
    { services: [], expected_services: ['chrono-llm-public', 'ornn-api'] },
    { services: ['chrono-llm-public', 'ornn-api'], expected_services: ['invalid/service'] },
  ])
    expect(() =>
      test.client.agents.updateServiceAccess('agent-one', body, { idempotencyKey: 'scope' }),
    ).toThrow(ValidationError);
  expect(test.calls).toEqual([]);
});
