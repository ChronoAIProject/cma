import { expect, it, vi } from 'vitest';
import { createClient, ProtocolError } from '../src/index.js';

const scope = {
  runtime_incarnation: 'incarnation',
  context_id: 'e9ed9ab9-02a9-4c1e-91f5-7e13ca191b24',
  client_id: 'client-one',
};
const operation = '78af4405-b644-44ab-b58d-53ce4d62acc0';
const receipt = {
  agent_id: 'agent-a',
  ...scope,
  state: 'user',
  revision: 1,
  lease_token: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
  expires_at: '2026-11-03T15:00:15Z',
  lease_duration_ms: 15000,
  operation_id: operation,
  operation_revision: 1,
  audit: [],
  audit_truncated_before: null,
};

it('sends the exact control operation and recovers its correlated receipt without repeating input', async () => {
  const fetch = vi
    .fn()
    .mockResolvedValueOnce(Response.json(receipt))
    .mockResolvedValueOnce(Response.json(receipt))
    .mockResolvedValueOnce(Response.json({ ...receipt, context_id: 'another-context' }));
  const client = createClient({ baseUrl: 'https://cma.example', fetch });
  const command = {
    ...scope,
    command: 'acquire' as const,
    operation_id: operation,
    expected_revision: 0,
  };
  const key = 'b2f66e8e-5ddd-4e96-b2e9-84eb55195664';
  expect(
    await client.agentBrowserControl.change('agent-a', { ...command, client_key: key }),
  ).toEqual(receipt);
  expect(JSON.parse(fetch.mock.calls[0]?.[1].body)).toEqual(command);
  expect(new Headers(fetch.mock.calls[0]?.[1].headers).get('x-cma-browser-client')).toBe(key);
  expect(
    await client.agentBrowserControl.read('agent-a', {
      ...scope,
      client_key: key,
      operation_id: operation,
    }),
  ).toEqual(receipt);
  expect(fetch.mock.calls[1]?.[0]).not.toContain(key);
  expect(fetch.mock.calls).toHaveLength(2);
  await expect(
    client.agentBrowserControl.read('agent-a', {
      ...scope,
      client_key: key,
      operation_id: operation,
    }),
  ).rejects.toThrow(ProtocolError);
});
